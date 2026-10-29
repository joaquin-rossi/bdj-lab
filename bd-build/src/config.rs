//! The declarative disc description (bd.yaml).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub disc: Disc,
    /// Needed when there are apps: they are signed.
    pub signing: Option<Signing>,
    /// What is stored on the disc.
    pub content: Content,
    /// How the disc is navigated (index.bdmv and what each title runs).
    pub navigation: Navigation,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Disc {
    /// Volume id (A-Z, 0-9, _; up to 30, the UDF limit), also the image file name.
    pub label: String,
    /// Disc name shown by players (BDMV/META/DL).
    pub name: Option<String>,
    /// Image players show for the disc, turned into a 640x360 JPEG (needs `name`).
    pub thumbnail: Option<PathBuf>,
    /// YYYY-MM-DD: every date on the disc is set to it (00:00 UTC), so the same config
    /// gives the same image. The current time if unset.
    pub date: Option<String>,
    /// ISO 639-2 code (eng, spa, ...) of the metadata and app names, and of the tracks
    /// that don't set one.
    pub language: String,
    pub org_id: String,
    pub disc_id: String,
}

/// Keys used to sign the apps, created on the first build and reused afterwards.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Signing {
    pub keystore: PathBuf,
    /// Keystore and key password, at least 6 characters (keytool's minimum). Better from
    /// the environment, as `${VARIABLE}`, than written here.
    pub password: String,
    /// Certificate organisation (O), followed by the org id as BD-J requires.
    pub organization: String,
    /// Certificate organisational unit (OU).
    pub unit: String,
    /// Certificate country (C), 2 letters.
    pub country: String,
    /// Certificate alternative name (BD-J tools always set one).
    pub email: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Content {
    #[serde(default)]
    pub apps: Vec<App>,
    pub clips: Vec<Clip>,
    /// Font subtitles are drawn with (they are turned into images).
    pub subtitle_font: Option<PathBuf>,
    #[serde(default)]
    pub fonts: Vec<Font>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct App {
    pub name: String,
    pub src: PathBuf,
    pub class: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Font {
    pub name: String,
    pub style: FontStyle,
    pub file: PathBuf,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum FontStyle {
    Plain,
    Bold,
    Italic,
    BoldItalic,
}

impl FontStyle {
    /// As written in dvb.fontindex (libbluray org/videolan/FontIndex.java).
    pub fn index_name(self) -> &'static str {
        match self {
            FontStyle::Plain => "PLAIN",
            FontStyle::Bold => "BOLD",
            FontStyle::Italic => "ITALIC",
            FontStyle::BoldItalic => "BOLD_ITALIC",
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Clip {
    pub name: String,
    pub file: PathBuf,
    #[serde(default)]
    pub video: Video,
    #[serde(default)]
    pub audio: Vec<Audio>,
    #[serde(default)]
    pub subtitles: Vec<Subtitle>,
    /// Times chapters start at, from the clip's start: [[HH:]MM:]SS[.fff].
    #[serde(default)]
    pub chapters: Vec<String>,
}

impl Clip {
    /// Chapter start times in milliseconds.
    pub fn chapter_times(&self) -> Result<Vec<u64>> {
        let mut times = Vec::new();
        for chapter in &self.chapters {
            let time = parse_time(chapter).with_context(|| format!("chapter {chapter:?}"))?;
            if times.last().is_some_and(|&last| time <= last) {
                bail!("chapter {chapter:?}: chapters must be in increasing order");
            }
            times.push(time);
        }
        Ok(times)
    }
}

/// Milliseconds of a time written [[HH:]MM:]SS[.fff].
fn parse_time(text: &str) -> Result<u64> {
    let (hms, fraction) = text.split_once('.').unwrap_or((text, ""));
    let parts: Vec<&str> = hms.split(':').collect();
    let digits = |s: &str| !s.is_empty() && s.len() <= 2 && s.bytes().all(|b| b.is_ascii_digit());
    if parts.len() > 3
        || !parts.iter().all(|p| digits(p))
        || fraction.len() > 3
        || !fraction.bytes().all(|b| b.is_ascii_digit())
    {
        bail!("not [[HH:]MM:]SS[.fff]");
    }
    let mut seconds = 0u64;
    for (i, part) in parts.iter().enumerate() {
        let value: u64 = part.parse()?;
        if i > 0 && value >= 60 {
            bail!("minutes and seconds go up to 59");
        }
        seconds = seconds * 60 + value;
    }
    let millis: u64 = format!("{fraction:0<3}").parse()?;
    Ok(seconds * 1000 + millis)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Subtitle {
    /// SRT file.
    pub file: PathBuf,
    /// ISO 639-2 code; disc.language if unset.
    pub language: Option<String>,
}

/// Format the clip is encoded to. Blu-ray allows 1080p at 23.976/24 fps and 720p at
/// 23.976/24/50/59.94 fps.
#[derive(Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Video {
    pub resolution: VideoResolution,
    pub fps: f64,
    /// x264 preset: slower ones compress better.
    pub preset: String,
    /// x264 constant rate factor, 0 to 51: lower is better quality and bigger.
    pub crf: u8,
}

impl Default for Video {
    fn default() -> Self {
        Video {
            resolution: VideoResolution::default(),
            fps: 23.976,
            preset: "fast".into(),
            crf: 23,
        }
    }
}

const PRESETS: &[&str] = &[
    "ultrafast",
    "superfast",
    "veryfast",
    "faster",
    "fast",
    "medium",
    "slow",
    "slower",
    "veryslow",
];

#[derive(Deserialize, Default, Clone, Copy, PartialEq)]
pub enum VideoResolution {
    #[default]
    #[serde(rename = "1920x1080")]
    Hd1080,
    #[serde(rename = "1280x720")]
    Hd720,
}

/// A frame rate as each tool writes it, and the matching 1 s GOP length.
pub struct Rate {
    pub ffmpeg: &'static str,
    pub tsmuxer: &'static str,
    pub keyint: u32,
}

impl Video {
    pub fn size(&self) -> (u32, u32) {
        match self.resolution {
            VideoResolution::Hd1080 => (1920, 1080),
            VideoResolution::Hd720 => (1280, 720),
        }
    }

    pub fn rate(&self) -> Result<Rate> {
        let rate = |ffmpeg, tsmuxer, keyint| {
            Ok(Rate {
                ffmpeg,
                tsmuxer,
                keyint,
            })
        };
        match ((self.fps * 1000.0).round() as u32, self.resolution) {
            (23976, _) => rate("24000/1001", "23.976", 24),
            (24000, _) => rate("24", "24", 24),
            (50000, VideoResolution::Hd720) => rate("50", "50", 50),
            (59940, VideoResolution::Hd720) => rate("60000/1001", "59.94", 60),
            _ => bail!(
                "{} fps is not a Blu-ray frame rate for {}x{}",
                self.fps,
                self.size().0,
                self.size().1
            ),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Audio {
    pub bitrate: String,
    pub channels: u32,
    /// ISO 639-2 code; disc.language if unset.
    pub language: Option<String>,
    /// Audio stream of the source file to take, counted from 0; the track's position if unset.
    pub stream: Option<usize>,
}

// Variants are declared like Kubernetes objects: `type: <variant>` plus a field named
// after the variant with its settings, e.g. `type: clip` + `clip: {...}`.

#[derive(Deserialize)]
#[serde(try_from = "RawTitle")]
pub struct Title {
    pub name: String,
    /// Name players show for it (BDMV/META/DL, needs disc.name).
    pub display_name: Option<String>,
    /// Not shown in the player's title list; still reachable by jumps.
    pub hidden: bool,
    pub kind: TitleKind,
}

pub enum TitleKind {
    App(AppTitle),
    Clip(ClipTitle),
    Playlist(PlaylistTitle),
}

/// Runs a BD-J app.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppTitle {
    pub name: String,
    #[serde(default)]
    pub resolution: Resolution,
}

/// Initial HAVi graphics resolution of a BD-J title.
#[derive(Deserialize, Default, Clone, Copy)]
pub enum Resolution {
    #[default]
    #[serde(rename = "1920x1080")]
    R1920x1080,
    #[serde(rename = "1280x720")]
    R1280x720,
    #[serde(rename = "720x576")]
    R720x576,
    #[serde(rename = "720x480")]
    R720x480,
    #[serde(rename = "960x540")]
    R960x540,
}

impl Resolution {
    /// initial_havi_config_id in the BDJO (libbluray bdjo/GraphicsResolution.java).
    pub fn havi_config_id(self) -> u8 {
        match self {
            Resolution::R1920x1080 => 1,
            Resolution::R1280x720 => 2,
            Resolution::R720x576 => 4,
            Resolution::R720x480 => 5,
            Resolution::R960x540 => 6,
        }
    }
}

/// Plays a clip, then jumps to a title.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClipTitle {
    pub name: String,
    pub then: String,
    /// User operations not allowed while it plays.
    #[serde(default)]
    pub mask: Vec<UserOp>,
    pub streams: Option<Streams>,
}

/// Plays a playlist, then jumps to a title.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlaylistTitle {
    pub name: String,
    pub then: String,
    /// User operations not allowed while it plays.
    #[serde(default)]
    pub mask: Vec<UserOp>,
    pub streams: Option<Streams>,
}

/// Tracks a title starts with, by language; without it the player picks them from its
/// own language settings.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Streams {
    /// Audio language; the player's choice if unset.
    pub audio: Option<String>,
    /// Subtitle language, or `off`. Off if unset too: the command that selects tracks
    /// always sets whether subtitles show.
    pub subtitles: Option<String>,
}

/// A user operation that a title can mask.
#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum UserOp {
    Menu,
    TitleSearch,
    ChapterSearch,
    TimeSearch,
    SkipNext,
    SkipPrev,
    Stop,
    Pause,
    Forward,
    Backward,
    AudioChange,
    AngleChange,
    Popup,
    SubtitleChange,
}

impl UserOp {
    /// Bit in the UO mask table, counted from its first, most significant bit
    /// (libbluray bdnav/uo_mask.c).
    fn bit(self) -> u32 {
        match self {
            UserOp::Menu => 0,
            UserOp::TitleSearch => 1,
            UserOp::ChapterSearch => 2,
            UserOp::TimeSearch => 3,
            UserOp::SkipNext => 4,
            UserOp::SkipPrev => 5,
            UserOp::Stop => 7,
            UserOp::Pause => 8,
            UserOp::Forward => 11,
            UserOp::Backward => 12,
            UserOp::AudioChange => 21,
            UserOp::AngleChange => 23,
            UserOp::Popup => 24,
            UserOp::SubtitleChange => 27,
        }
    }
}

/// UO mask table (64 bits, first bit most significant) of a list of operations.
pub fn uo_mask(ops: &[UserOp]) -> u64 {
    ops.iter().fold(0, |mask, op| mask | 1 << (63 - op.bit()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTitle {
    name: String,
    display_name: Option<String>,
    #[serde(default)]
    hidden: bool,
    #[serde(rename = "type")]
    kind: TitleType,
    app: Option<AppTitle>,
    clip: Option<ClipTitle>,
    playlist: Option<PlaylistTitle>,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum TitleType {
    App,
    Clip,
    Playlist,
}

impl TryFrom<RawTitle> for Title {
    type Error = String;

    fn try_from(raw: RawTitle) -> Result<Self, String> {
        let kind = match (raw.kind, raw.app, raw.clip, raw.playlist) {
            (TitleType::App, Some(app), None, None) => TitleKind::App(app),
            (TitleType::Clip, None, Some(clip), None) => TitleKind::Clip(clip),
            (TitleType::Playlist, None, None, Some(playlist)) => TitleKind::Playlist(playlist),
            (kind, ..) => {
                let kind = match kind {
                    TitleType::App => "app",
                    TitleType::Clip => "clip",
                    TitleType::Playlist => "playlist",
                };
                return Err(format!(
                    "title {:?}: `type: {kind}` needs `{kind}` and no other title type",
                    raw.name
                ));
            }
        };
        Ok(Title {
            name: raw.name,
            display_name: raw.display_name,
            hidden: raw.hidden,
            kind,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Navigation {
    pub first_play: String,
    pub top_menu: String,
    /// Playlists beyond the one each clip gets.
    #[serde(default)]
    pub playlists: Vec<Playlist>,
    pub titles: Vec<Title>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Playlist {
    pub name: String,
    pub items: Vec<PlaylistItem>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlaylistItem {
    pub clip: String,
    /// Starts a chapter (the first item always does).
    #[serde(default)]
    pub chapter: bool,
}

impl Playlist {
    /// Indexes of the items that start a chapter.
    pub fn chapters(&self) -> Vec<usize> {
        (0..self.items.len())
            .filter(|&i| i == 0 || self.items[i].chapter)
            .collect()
    }
}

impl Config {
    /// Parses and validates the config, so that a bad one fails before building anything.
    /// Each `${NAME}` in it is replaced by the environment variable NAME first.
    pub fn parse(text: &str) -> Result<Self> {
        let mut config: Config = serde_norway::from_str(&substitute(text)?)?;
        let disc = &mut config.disc;
        disc.org_id.make_ascii_lowercase();
        disc.disc_id.make_ascii_lowercase();
        let is_hex = |s: &str, len| s.len() == len && s.chars().all(|c| c.is_ascii_hexdigit());
        if !is_hex(&disc.org_id, 8) {
            bail!("disc.org_id must be 8 hex digits");
        }
        if !is_hex(&disc.disc_id, 32) {
            bail!("disc.disc_id must be 32 hex digits");
        }
        let label_char = |c: char| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_';
        if disc.label.is_empty() || disc.label.len() > 30 || !disc.label.chars().all(label_char) {
            bail!("disc.label must be 1 to 30 characters among A-Z, 0-9 and _");
        }
        config.date().context("disc.date")?;
        let metadata = config.disc.thumbnail.is_some()
            || config
                .navigation
                .titles
                .iter()
                .any(|t| t.display_name.is_some());
        if metadata && config.disc.name.is_none() {
            bail!("disc.thumbnail and title display names need disc.name");
        }
        if !is_language(&config.disc.language) {
            bail!("disc.language must be an ISO 639-2 code, 3 lowercase letters");
        }
        match &config.signing {
            Some(signing) => {
                if signing.password.len() < 6 {
                    bail!("signing.password must be at least 6 characters");
                }
                let country = &signing.country;
                if country.len() != 2 || !country.chars().all(|c| c.is_ascii_alphabetic()) {
                    bail!("signing.country must be 2 letters");
                }
            }
            None if !config.content.apps.is_empty() => bail!("apps need signing"),
            None => {}
        }

        unique("app", config.content.apps.iter().map(|a| &a.name))?;
        unique("clip", config.content.clips.iter().map(|c| &c.name))?;
        unique(
            "playlist",
            config.navigation.playlists.iter().map(|p| &p.name),
        )?;
        unique("title", config.navigation.titles.iter().map(|t| &t.name))?;

        for clip in &config.content.clips {
            let context = || format!("clip {:?}", clip.name);
            clip.video.rate().with_context(context)?;
            if !PRESETS.contains(&clip.video.preset.as_str()) {
                bail!(
                    "{}: video.preset must be one of {}",
                    context(),
                    PRESETS.join(", ")
                );
            }
            clip.chapter_times().with_context(context)?;
            if clip.video.crf > 51 {
                bail!("{}: video.crf must be 0 to 51", context());
            }
            let audio: Vec<_> = clip
                .audio
                .iter()
                .map(|a| config.track_language(&a.language))
                .collect();
            let subtitles: Vec<_> = clip
                .subtitles
                .iter()
                .map(|s| config.track_language(&s.language))
                .collect();
            for (what, languages) in [("audio", &audio), ("subtitles", &subtitles)] {
                if let Some(l) = languages.iter().find(|l| !is_language(l)) {
                    bail!(
                        "{}: {what} language {l:?} must be an ISO 639-2 code, 3 lowercase letters",
                        context()
                    );
                }
            }
            if !clip.subtitles.is_empty() && config.content.subtitle_font.is_none() {
                bail!("{}: subtitles need content.subtitle_font", context());
            }
        }
        for playlist in &config.navigation.playlists {
            if playlist.items.is_empty() {
                bail!("playlist {:?} has no items", playlist.name);
            }
            for item in &playlist.items {
                config
                    .clip_index(&item.clip)
                    .with_context(|| format!("playlist {:?}", playlist.name))?;
            }
        }
        let mut masks = HashMap::new();
        for title in &config.navigation.titles {
            config
                .check_title(title)
                .with_context(|| format!("title {:?}", title.name))?;
            // A playlist's mask is recorded in it, so all its titles must agree.
            if let Some((playlist, mask)) = config.playback(title)?
                && masks.insert(playlist, mask).is_some_and(|m| m != mask)
            {
                bail!(
                    "title {:?}: titles playing the same clip or playlist need the same mask",
                    title.name
                );
            }
        }
        config
            .title_number(&config.navigation.first_play)
            .context("navigation.first_play")?;
        config
            .title_number(&config.navigation.top_menu)
            .context("navigation.top_menu")?;
        Ok(config)
    }

    /// A track's language, or disc.language.
    pub fn track_language<'a>(&'a self, language: &'a Option<String>) -> &'a str {
        language.as_deref().unwrap_or(&self.disc.language)
    }

    /// The clips a clip or playlist title plays.
    fn title_clips(&self, title: &Title) -> Result<Vec<&Clip>> {
        let clip =
            |name| -> Result<&Clip> { Ok(&self.content.clips[self.clip_index(name)? as usize]) };
        match &title.kind {
            TitleKind::App(_) => Ok(Vec::new()),
            TitleKind::Clip(c) => Ok(vec![clip(&c.name)?]),
            TitleKind::Playlist(p) => {
                let n = self.playlist_number(&p.name)? as usize - self.content.clips.len() - 1;
                self.navigation.playlists[n]
                    .items
                    .iter()
                    .map(|item| clip(&item.clip))
                    .collect()
            }
        }
    }

    /// Track numbers (from 1) a title selects with its `streams`: audio (None: the
    /// player's choice) and subtitles (None: off). None without `streams`.
    pub fn stream_numbers(&self, title: &Title) -> Result<Option<(Option<u16>, Option<u16>)>> {
        let streams = match &title.kind {
            TitleKind::App(_) => None,
            TitleKind::Clip(c) => c.streams.as_ref(),
            TitleKind::Playlist(p) => p.streams.as_ref(),
        };
        let Some(streams) = streams else {
            return Ok(None);
        };
        let clips = self.title_clips(title)?;
        // One command selects the tracks for the whole playlist, so every clip needs the
        // language at the same position.
        let number = |audio: bool, language: &str| -> Result<u16> {
            let what = if audio { "audio" } else { "subtitles" };
            let mut number = None;
            for clip in &clips {
                let tracks: Vec<&str> = if audio {
                    clip.audio
                        .iter()
                        .map(|a| self.track_language(&a.language))
                        .collect()
                } else {
                    clip.subtitles
                        .iter()
                        .map(|s| self.track_language(&s.language))
                        .collect()
                };
                let Some(i) = tracks.iter().position(|l| *l == language) else {
                    bail!("clip {:?} has no {language} {what}", clip.name);
                };
                if number.is_some_and(|n| n != i) {
                    bail!("its clips have {language} {what} at different positions");
                }
                number = Some(i);
            }
            Ok(number.unwrap_or(0) as u16 + 1)
        };
        let audio = match &streams.audio {
            Some(language) => Some(number(true, language)?),
            None => None,
        };
        let subtitles = match streams.subtitles.as_deref() {
            Some(language) if language != "off" => Some(number(false, language)?),
            _ => None,
        };
        Ok(Some((audio, subtitles)))
    }

    /// The playlist a clip or playlist title plays, and its UO mask.
    pub fn playback(&self, title: &Title) -> Result<Option<(u16, u64)>> {
        Ok(match &title.kind {
            TitleKind::App(_) => None,
            TitleKind::Clip(clip) => Some((self.clip_index(&clip.name)?, uo_mask(&clip.mask))),
            TitleKind::Playlist(p) => Some((self.playlist_number(&p.name)?, uo_mask(&p.mask))),
        })
    }

    /// Checks that what a title refers to exists.
    fn check_title(&self, title: &Title) -> Result<()> {
        self.stream_numbers(title)?;
        match &title.kind {
            TitleKind::App(app) => {
                self.app_index(&app.name)?;
            }
            TitleKind::Clip(clip) => {
                self.clip_index(&clip.name)?;
                self.title_number(&clip.then)?;
            }
            TitleKind::Playlist(playlist) => {
                self.playlist_number(&playlist.name)?;
                self.title_number(&playlist.then)?;
            }
        }
        Ok(())
    }

    /// The time to record on the disc: `disc.date`, or now.
    pub fn date(&self) -> Result<SystemTime> {
        let Some(date) = &self.disc.date else {
            return Ok(SystemTime::now());
        };
        let parts: Vec<_> = date.split('-').collect();
        let digits = |s: &str, len| s.len() == len && s.bytes().all(|b| b.is_ascii_digit());
        let [y, m, d] = parts[..] else {
            bail!("{date:?} is not YYYY-MM-DD");
        };
        if !digits(y, 4) || !digits(m, 2) || !digits(d, 2) {
            bail!("{date:?} is not YYYY-MM-DD");
        }
        let (y, m, d): (u32, u32, u32) = (y.parse()?, m.parse()?, d.parse()?);
        let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
        let month_days = [
            31,
            if leap { 29 } else { 28 },
            31,
            30,
            31,
            30,
            31,
            31,
            30,
            31,
            30,
            31,
        ];
        // From 1980: zip entry times in the JAR can't go earlier.
        if !(1980..=9999).contains(&y)
            || !(1..=12).contains(&m)
            || d < 1
            || d > month_days[m as usize - 1]
        {
            bail!("{date:?} is not a date from 1980 to 9999");
        }
        // Days since 1970-01-01 (Howard Hinnant's days_from_civil).
        let (y, m, d) = (y as i64 - (m <= 2) as i64, m as i64, d as i64);
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
        let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
        Ok(UNIX_EPOCH + Duration::from_secs(days as u64 * 86400))
    }

    pub fn org_id(&self) -> Result<u32> {
        u32::from_str_radix(&self.disc.org_id, 16).context("disc.org_id is not hex")
    }

    pub fn disc_id(&self) -> Result<[u8; 16]> {
        let id = u128::from_str_radix(&self.disc.disc_id, 16).context("disc.disc_id is not hex")?;
        Ok(id.to_be_bytes())
    }

    /// App index, which is also its JAR and BDJO number.
    pub fn app_index(&self, name: &str) -> Result<u16> {
        match self.content.apps.iter().position(|a| a.name == name) {
            Some(i) => Ok(i as u16),
            None => bail!("unknown app {name:?}"),
        }
    }

    /// Clip index, which is also its playlist and m2ts number.
    pub fn clip_index(&self, name: &str) -> Result<u16> {
        match self.content.clips.iter().position(|c| c.name == name) {
            Some(i) => Ok(i as u16),
            None => bail!("unknown clip {name:?}"),
        }
    }

    /// Playlist number: after the clips' own playlists and the background one.
    pub fn playlist_number(&self, name: &str) -> Result<u16> {
        match self
            .navigation
            .playlists
            .iter()
            .position(|p| p.name == name)
        {
            Some(i) => Ok((self.content.clips.len() + 1 + i) as u16),
            None => bail!("unknown playlist {name:?}"),
        }
    }

    /// Title number, counted from 1.
    pub fn title_number(&self, name: &str) -> Result<u16> {
        match self.navigation.titles.iter().position(|t| t.name == name) {
            Some(i) => Ok(i as u16 + 1),
            None => bail!("unknown title {name:?}"),
        }
    }
}

fn is_language(code: &str) -> bool {
    code.len() == 3 && code.bytes().all(|b| b.is_ascii_lowercase())
}

/// The config text with each `${NAME}` replaced by the environment variable NAME. Values
/// go in as they are, so quote them in the YAML if they may hold special characters.
fn substitute(text: &str) -> Result<String> {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(after.len());
        if end == 0 || !after[end..].starts_with('}') {
            bail!(
                "expected ${{NAME}} in {:?}",
                rest[start..].lines().next().unwrap_or("")
            );
        }
        let name = &after[..end];
        let Ok(value) = std::env::var(name) else {
            bail!("${{{name}}}: environment variable not set");
        };
        out.push_str(&value);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Fails if two names in a list are equal: lookups by name would silently use the first.
fn unique<'a>(what: &str, names: impl Iterator<Item = &'a String>) -> Result<()> {
    let mut seen = HashSet::new();
    for name in names {
        if !seen.insert(name) {
            bail!("two {what}s are named {name:?}");
        }
    }
    Ok(())
}
