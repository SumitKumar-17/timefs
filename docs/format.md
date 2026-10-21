# On-disk format

* The format is **canonical**: one logical object has exactly one byte encoding, because the encoding is hashed.
* Never reorder fields, add padding or change integer widths without bumping the format version.
* Implemented in `timefs-core` (`codec`, `object`, `store`, `refs`).
* Two repository layouts share the same object encodings:
  * **format 1, loose**: one file per object and per ref.
  * **format 2, pack** (default since M7): objects and refs as records in an append-only log.

## Repository layout, format 1 (loose)

```text
<repo>/
  FORMAT              "timefs-repo 1\n"
  LOCK                empty file; flock'd while a daemon / exclusive command runs
  HEAD                "ref: refs/heads/<branch>\n"
  objects/
    ab/cdef…          immutable object, name = lowercase hex BLAKE3 (2 + 62 chars)
    tmp/              in-flight writes (renamed into place when complete)
  refs/
    heads/<branch>    "<64 hex>\n"  branch tip commit
    wip/<branch>      "<64 hex>\n"  checkpoint commit for a mounted branch
  timefs.sock         control socket (exists only while a daemon runs)
```

## Repository layout, format 2 (pack)

```text
<repo>/
  FORMAT              "timefs-repo 2\n"
  LOCK                as in format 1
  HEAD                as in format 1
  pack/
    WRITER            empty file; flock'd by the one process allowed to append
    seg-00000001.log  segments, appended in ascending order, never rewritten
    seg-00000002.log  a new segment starts when the active one would exceed 256 MiB
  timefs.sock         as in format 1
```

### Pack records

```text
record  = magic:u8 (0xA5) | type:u8 | len:u32 LE | payload[len] | check[8]
check   = first 8 bytes of BLAKE3(type | len | payload)

type 1  object   payload = hash[32] | kind:u8 | object payload (as in the envelope below)
type 2  ref set  payload = hash[32] | ref name (UTF-8, e.g. "heads/main")
type 3  ref del  payload = ref name
type 4  synced   payload = upto:u64 LE  (everything before this offset was fsynced)
```

* Replay: segments in order; later object records for the same hash win; ref records
  set or delete in log order.
* Validity: a segment is read up to its first record that is incomplete or fails `check`.
  Only the last segment may end that way (a torn write); sealed segments must be complete.
* A `synced` marker is written in front of the first record after each successful
  `fdatasync`. A damaged record before a valid marker that covers it is corruption
  (opening fails); without such a marker it is a torn tail and is trimmed by the next writer.
* Ordering: a ref record is always appended after the objects it names, so trimming a torn
  tail can drop a ref but never leave one pointing at a lost object.

## Primitive encodings

* All integers little-endian, fixed width unless noted.
* `varint`: unsigned LEB128, at most 10 bytes, **minimal encoding required** (a decoder
  rejects non-minimal forms so the encoding stays canonical).
* `bytes`: `varint length` followed by that many bytes.
* `hash`: 32 raw bytes (BLAKE3-256).

## Object envelope

```text
object   = kind:u8 || payload
hash     = BLAKE3( kind || payload )
on disk  = the same bytes (kind || payload), file named by hash
```

On read the store may re-hash and compare with the file name (`--verify`, always on in `fsck`).

| kind | name   | payload |
|------|--------|---------|
| 1 | Blob   | raw bytes (a block of file data, or a symlink target) |
| 2 | File   | `varint n` then n × (`hash`, `varint len`) |
| 3 | Tree   | `varint n` then n × entry, sorted by `name` bytewise ascending, names unique |
| 4 | Commit | see below |
| 5 | FileIndex | `varint n` then n × (`hash` of a File page, `varint size`, `varint extents`) |

### Tree entry

```text
name:bytes   (non-empty, no '/', no NUL, not "." or "..")
kind:u8      0 = regular file, 1 = directory, 2 = symlink
mode:u32     permission bits only (mode & 0o7777)
uid:u32
gid:u32
size:u64     file: logical size · symlink: target length · dir: 0
mtime:i64    nanoseconds since the Unix epoch
hash         file → File object · dir → Tree object · symlink → Blob object
```

An empty file is a `File` object with zero extents.

### Long files and holes

* A tree entry of kind file names a `File` object or, for more than 1024 extents, a
  `FileIndex` whose pages are `File` objects of at most 1024 extents each. Page `size` and
  `extents` must match the page, otherwise the file is corrupt.
* An extent whose hash is all zero bytes is a **hole**: `len` zero bytes, no blob behind it.
* Both were added before the first release; readers of earlier development builds cannot
  read files that use them.

### Commit

```text
tree:hash
nparents:varint        (0 = root commit, 1 = normal, 2 = merge; >2 reserved)
parents: nparents × hash
time:i64               nanoseconds since the Unix epoch
author:bytes           UTF-8
message:bytes          UTF-8
```

## Refs

* Names: `heads/<branch>` for branch tips, `wip/<branch>` for checkpoints.
* Format 2 stores them as log records (above). Format 1 stores each as a file:
* Plain text, one line: 64 lowercase hex characters and `\n`.
* Written via temp file, `fsync`, `rename`, `fsync(parent dir)`.
* Branch names: `[A-Za-z0-9._-/]`, no `..`, no leading/trailing `/`, no `.lock` suffix.

## Compatibility policy

* `FORMAT` carries the layout version (1 = loose, 2 = pack); object encodings are the same in both.
* A reader refuses repositories with a higher version.
* Any change to the bytes of an existing object kind needs a new version and a documented migration.
