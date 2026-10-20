# On-disk format (version 1)

The format is **canonical**: a given logical object has exactly one byte encoding, because
the encoding is what gets hashed. Never reorder fields, add padding, or change integer
widths without bumping the format version.

## Repository layout

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

## Primitive encodings

* All integers little-endian, fixed width unless noted.
* `varint` — unsigned LEB128, at most 10 bytes, **minimal encoding required** (a decoder
  rejects non-minimal forms so the encoding stays canonical).
* `bytes` — `varint length` followed by that many bytes.
* `hash` — 32 raw bytes (BLAKE3-256).

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

Plain text, one line: 64 lowercase hex characters and `\n`. Written via temp file →
`fsync` → `rename` → `fsync(parent dir)`. Branch names: `[A-Za-z0-9._-/]`, no `..`, no
leading/trailing `/`, no `.lock` suffix.

## Compatibility policy

`FORMAT` carries the major version. A reader refuses repositories with a higher version.
Any change that alters the bytes of an existing object kind requires a new version and a
documented migration.
