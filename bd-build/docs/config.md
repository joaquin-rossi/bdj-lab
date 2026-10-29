# bd.yaml

The disc `bd-build` makes, described in four sections:

- [`disc`](#disc): what identifies the disc.
- [`signing`](#signing): the keys that sign the apps.
- [`content`](#content): what is stored on the disc (apps, clips, fonts).
- [`navigation`](#navigation): how the disc is navigated (titles, playlists).

```yaml
disc:
  label: MY_DISC
  language: eng
  org_id: "cafebabe"
  disc_id: "0123456789abcdef0123456789abcdef"

content:
  clips:
    - name: movie
      file: assets/movie.mkv
      audio:
        - { bitrate: 448k, channels: 6 }

navigation:
  first_play: movie
  top_menu: movie
  titles:
    - name: movie
      type: clip
      clip:
        name: movie
        then: movie
```

## General rules

- Paths are relative to the `bd.yaml`.
- `${NAME}` is replaced by the environment variable `NAME` before parsing.
  Quote the value if it may hold YAML special characters.
- Names identify apps, clips, playlists and titles, and must be unique within
  each list.
- Unknown fields are errors, and the whole config is checked before anything is
  built.

## disc

```yaml
disc:
  label: MY_DISC
  name: "My Disc"
  thumbnail: assets/cover.png
  date: 2026-10-29
  language: eng
  org_id: "cafebabe"
  disc_id: "0123456789abcdef0123456789abcdef"
```

| Option      | Required | Description                                                                                                                                      |
| ----------- | -------- | ------------------------------------------------------------------------------------------------------------------------------------------------ |
| `label`     | yes      | Volume id: 1 to 30 characters among `A-Z`, `0-9` and `_`. Also the image file name, `build/<label>.iso`.                                         |
| `name`      | no       | Disc name players show.                                                                                                                          |
| `thumbnail` | no       | Image players show for the disc. Any format ffmpeg reads; it becomes a 640x360 JPEG. Needs `name`.                                               |
| `date`      | no       | `YYYY-MM-DD`, 1980 to 9999. Every date on the disc is set to it (00:00 UTC), so the same config gives the same image. The current time if unset. |
| `language`  | yes      | ISO 639-2 code (`eng`, `spa`, ...) of the metadata and app names, and of the tracks that don't set one.                                          |
| `org_id`    | yes      | Organisation id, 8 hex digits. Also goes into the signing certificate.                                                                           |
| `disc_id`   | yes      | Disc id, 32 hex digits.                                                                                                                          |

## signing

The keys that sign the apps' JARs. Required when there are apps, unused
otherwise.

```yaml
signing:
  keystore: assets/keys/bd.jks
  password: "${BD_STOREPASS}"
  organization: "My Studio"
  unit: "Codesigning Department"
  country: US
  email: keys@example.com
```

| Option         | Description                                                                                                                                                   |
| -------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `keystore`     | Keystore file. Created on the first build and reused afterwards.                                                                                              |
| `password`     | Keystore and key password, at least 6 characters. Better from the environment, as `${VARIABLE}`, than written here.                                           |
| `organization` | Certificate organisation (O). The certificates are named `<organization> Root` and `<organization> App`, and `disc.org_id` is appended to O as BD-J requires. |
| `unit`         | Certificate organisational unit (OU).                                                                                                                         |
| `country`      | Certificate country (C), 2 letters.                                                                                                                           |
| `email`        | Certificate alternative name.                                                                                                                                 |

All options are required.

A keystore made for another organization, unit, country or org id can't be
reused: the build fails, and deleting the keystore makes a new one.

## content

What is stored on the disc.

```yaml
content:
  apps:
    - name: menu
      src: src
      class: Menu

  clips:
    - name: ep1
      file: assets/ep1.mkv
      video: { preset: slow, crf: 20 }
      audio:
        - { language: jpn, bitrate: 448k, channels: 6 }
        - { language: eng, bitrate: 448k, channels: 6 }
      subtitles:
        - { language: eng, file: assets/ep1.eng.srt }
      chapters: ["1:30", "22:10.5"]

  subtitle_font: assets/fonts/DejaVuSans.ttf

  fonts:
    - name: Sans
      style: bold
      file: assets/fonts/DejaVuSans-Bold.ttf
```

| Option          | Required               | Description                                                 |
| --------------- | ---------------------- | ----------------------------------------------------------- |
| `apps`          | no                     | BD-J apps.                                                  |
| `clips`         | yes                    | Videos, with their audio and subtitle tracks.               |
| `subtitle_font` | if there are subtitles | Font subtitles are drawn with: they are turned into images. |
| `fonts`         | no                     | Fonts for the apps.                                         |

### apps

Each app is compiled, signed and stored as `BDMV/JAR/NNNNN.jar`. A title of
type `app` runs it.

| Option  | Description                                              |
| ------- | -------------------------------------------------------- |
| `name`  | App name.                                                |
| `src`   | Directory with its `.java` files, compiled for Java 1.3. |
| `class` | Xlet class entrypoint.                                   |

All options are required. Apps can use `bdbuild.Titles`, a generated class with
each title's number (see [navigation](#titles)).

### clips

Each clip is encoded and gets its own playlist.

| Option      | Required | Description                                                                                                               |
| ----------- | -------- | ------------------------------------------------------------------------------------------------------------------------- |
| `name`      | yes      | Clip name.                                                                                                                |
| `file`      | yes      | Source video: anything ffmpeg reads. Its first video stream is used.                                                      |
| `video`     | no       | Format and quality of the video, below.                                                                                   |
| `audio`     | no       | Audio tracks, below.                                                                                                      |
| `subtitles` | no       | Subtitle tracks, below.                                                                                                   |
| `chapters`  | no       | Times chapters start at, from the clip's start, as `[[HH:]MM:]SS[.fff]` and in increasing order. The start is always one. |

#### video

| Option       | Default     | Description                                                              |
| ------------ | ----------- | ------------------------------------------------------------------------ |
| `resolution` | `1920x1080` | `1920x1080` or `1280x720`. The source is scaled to fit, with black bars. |
| `fps`        | `23.976`    | `23.976` or `24`. `50` and `59.94` only at `1280x720`.                   |
| `preset`     | `fast`      | x264 preset, `ultrafast` to `veryslow`: slower ones compress better.     |
| `crf`        | `23`        | x264 constant rate factor, 0 to 51: lower is better quality and bigger.  |

#### audio

Each track is encoded as AC3.

| Option     | Required | Description                                                                        |
| ---------- | -------- | ---------------------------------------------------------------------------------- |
| `bitrate`  | yes      | E.g. `448k`.                                                                       |
| `channels` | yes      | E.g. `2` or `6`.                                                                   |
| `language` | no       | ISO 639-2 code; `disc.language` if unset.                                          |
| `stream`   | no       | Audio stream of the source to take, counted from 0; the track's position if unset. |

#### subtitles

| Option     | Required | Description                               |
| ---------- | -------- | ----------------------------------------- |
| `file`     | yes      | SRT file, UTF-8.                          |
| `language` | no       | ISO 639-2 code; `disc.language` if unset. |

### fonts

Stored in `BDMV/AUXDATA`, where apps load them by name with
`org.dvb.ui.FontFactory`.

| Option  | Description                                 |
| ------- | ------------------------------------------- |
| `name`  | Name apps ask for.                          |
| `style` | `plain`, `bold`, `italic` or `bold_italic`. |
| `file`  | TrueType or OpenType font.                  |

All options are required.

## navigation

How the disc is navigated.

```yaml
navigation:
  first_play: warning
  top_menu: menu

  playlists:
    - name: episodes
      items:
        - clip: ep1
        - clip: ep2
          chapter: true

  titles:
    - name: menu
      type: app
      app:
        name: menu

    - name: warning
      hidden: true
      type: clip
      clip:
        name: piracy-warning
        then: menu
        mask: [menu, skip_next, forward]

    - name: episodes
      display_name: "All episodes"
      type: playlist
      playlist:
        name: episodes
        then: menu
        streams: { audio: jpn, subtitles: eng }
```

| Option       | Required | Description                             |
| ------------ | -------- | --------------------------------------- |
| `first_play` | yes      | Title played when the disc starts.      |
| `top_menu`   | yes      | Title the remote's menu button goes to. |
| `playlists`  | no       | Clips played one after another.         |
| `titles`     | yes      | What the disc can play.                 |

### playlists

Each clip already has its own playlist; these are for playing several in a row.

| Option  | Description                                                                                                                                       |
| ------- | ------------------------------------------------------------------------------------------------------------------------------------------------- |
| `name`  | Playlist name.                                                                                                                                    |
| `items` | Clips, in order. Each one is `clip` (a clip name) and optionally `chapter: true`, so that its start begins a chapter. The first item always does. |

The chapters of each clip are chapters of the playlist too.

### titles

Titles are numbered from 1 in this order. Apps jump to them by number, through
the generated class `bdbuild.Titles` (`"piracy-warning"` becomes
`Titles.PIRACY_WARNING`).

| Option         | Required | Description                                                                           |
| -------------- | -------- | ------------------------------------------------------------------------------------- |
| `name`         | yes      | Title name.                                                                           |
| `display_name` | no       | Name players show. Needs `disc.name`.                                                 |
| `hidden`       | no       | Not shown in the player's title list, but still reachable by jumps. `false` if unset. |
| `type`         | yes      | `app`, `clip` or `playlist`, with a field of the same name, below.                    |

#### app

Runs a BD-J app.

| Option       | Required | Description                                                                                        |
| ------------ | -------- | -------------------------------------------------------------------------------------------------- |
| `name`       | yes      | App name.                                                                                          |
| `resolution` | no       | Initial graphics resolution: `1920x1080` (default), `1280x720`, `960x540`, `720x576` or `720x480`. |

#### clip and playlist

Plays a clip or a playlist, then jumps to a title.

| Option    | Required | Description                                                                                                     |
| --------- | -------- | --------------------------------------------------------------------------------------------------------------- |
| `name`    | yes      | Clip or playlist name.                                                                                          |
| `then`    | yes      | Title to jump to when it ends.                                                                                  |
| `mask`    | no       | User operations not allowed while it plays, below. Titles playing the same clip or playlist need the same mask. |
| `streams` | no       | Tracks it starts with, below.                                                                                   |

#### mask

Any of: `menu`, `title_search`, `chapter_search`, `time_search`, `skip_next`,
`skip_prev`, `stop`, `pause`, `forward`, `backward`, `audio_change`,
`angle_change`, `popup`, `subtitle_change`.

For example, a warning that can't be skipped:
`[menu, title_search, chapter_search, time_search, skip_next, forward]`.

#### streams

The audio and subtitle tracks a title starts with, by language. Without it, the
player picks them from its own language settings. Users can change them with
the remote anyway, unless masked.

| Option      | Description                                    |
| ----------- | ---------------------------------------------- |
| `audio`     | Audio language. The player's choice if unset.  |
| `subtitles` | Subtitle language, or `off`. Off if unset too. |

Every clip the title plays needs that language, at the same position among its
tracks.
