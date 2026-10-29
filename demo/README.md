# demo

An example disc: a piracy warning, a video and a Java menu.

```sh
mise exec -- ./build.sh   # build/BDJ_LAB_DEMO.iso
mise exec -- ./run.sh     # opens it in VLC, which needs Java 8 for the menu
```

The keystore in `assets/keys/` and its password in `mise.toml` are public on
purpose: they only sign this demo. Don't use them for a real disc.

The font is DejaVu Sans, under [its own license](assets/fonts/LICENSE-DejaVu).
