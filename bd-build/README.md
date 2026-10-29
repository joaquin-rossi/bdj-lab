# bd-build

Builds a Blu-ray image from a declarative config.

```sh
bd-build [bd.yaml]   # writes build/<disc.label>.iso next to the bd.yaml
```

## What it does

1. Compiles the BD-J apps against libbluray's BD-J API, and signs their JARs.
2. Generates the navigation and the metadata.
3. Encodes the clips with ffmpeg and muxes them with tsMuxeR.
4. Writes the UDF 2.50 image with `udf-image`.

Encodes are kept in `build/cache/` and reused while a clip and its settings
don't change.

## bd.yaml

See [`docs/config.md`](docs/config.md).

## Requirements

- Java 8
- ffmpeg
- tsMuxeR
- libbluray's JAR (`/usr/share/java/libbluray-j2se-*.jar`, or set `LIBBLURAY_JAR`)
