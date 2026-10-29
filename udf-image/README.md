# udf-image

Writes a directory as a UDF 2.50 image laid out like a Blu-ray disc: read-only,
with a metadata partition and its mirror. It needs no root and no external
tools.

```rust
udf_image::write(Path::new("disc"), "MY_DISC", SystemTime::now(), Path::new("disc.iso"))?;
```

- The time is used for every date on the disc, so the same inputs give the same image.
- File names must be ASCII.
- Symlinks are written as what they point to.

It follows ECMA-167 and OSTA UDF 2.50, and is checked with libudfread (what
libbluray and VLC read images with), `udfinfo` and the Linux UDF driver.
