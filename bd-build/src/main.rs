//! bd-build: builds a BD-J Blu-ray ISO from a declarative bd.yaml.
//!
//! usage: bd-build [config]   (default bd.yaml; output in build/ next to it)

mod bdmv;
mod config;
mod mpls;

use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};

use config::Config;

/// BD-J API packages taken from libbluray's implementation to compile apps against;
/// the rest of its jar (org.videolan, bundled libraries) doesn't exist on real players.
const BDJ_API: &[&str] = &[
    "com/aacsla",
    "javax/media",
    "javax/microedition",
    "javax/tv",
    "org/bluray",
    "org/blurayx",
    "org/davic",
    "org/dvb",
    "org/havi",
];

/// Where distributions install libbluray's BD-J jar; LIBBLURAY_JAR overrides it.
const LIBBLURAY_JAR_DIR: &str = "/usr/share/java";

const BDJ_SIGN: &str = include_str!("../java/BDJSign.java");

const JAVAC_FLAGS: &[&str] = &["-source", "1.3", "-target", "1.3", "-Xlint:-options"];

fn main() {
    let config = std::env::args().nth(1).unwrap_or_else(|| "bd.yaml".into());
    if let Err(e) = build(Path::new(&config)) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn build(config_path: &Path) -> Result<()> {
    let text = fs::read_to_string(config_path)
        .with_context(|| format!("reading {}", config_path.display()))?;
    let config =
        Config::parse(&text).with_context(|| format!("parsing {}", config_path.display()))?;
    let base = config_path.parent().unwrap_or(Path::new("")).to_path_buf();
    let out = base.join("build");
    let disc = out.join("disc");
    let time = config.date()?;
    check_inputs(&config, &base)?;

    // Everything is rebuilt but the encoded clips in build/cache.
    if out.exists() {
        for entry in fs::read_dir(&out)? {
            let path = entry?.path();
            if path.file_name() != Some(OsStr::new("cache")) {
                remove(&path)?;
            }
        }
    }
    for d in [
        "BDMV/AUXDATA",
        "BDMV/BACKUP",
        "BDMV/BDJO",
        "BDMV/CLIPINF",
        "BDMV/JAR",
        "BDMV/PLAYLIST",
        "BDMV/STREAM",
        "CERTIFICATE/BACKUP",
    ] {
        fs::create_dir_all(disc.join(d))?;
    }

    // Signing is validated to be there when there are apps.
    if let Some(signing) = config
        .signing
        .as_ref()
        .filter(|_| !config.content.apps.is_empty())
    {
        println!("==> 1. Compiling apps...");
        compile_apps(&config, &base, &out)?;

        println!("==> 2. Signing JARs and exporting root certificate...");
        sign_apps(&config, signing, &base, &out, &disc, time)?;
    }

    println!("==> 3. Generating BDMV navigation files and metadata...");
    write_navigation(&config, &disc)?;
    if let Some(thumbnail) = &config.disc.thumbnail {
        write_thumbnail(&base.join(thumbnail), &disc)?;
    }

    println!("==> 4. Generating AUXDATA...");
    write_fonts(&config, &base, &disc)?;

    println!("==> 5. Encoding clips...");
    let media = out.join("media");
    fs::create_dir_all(&media)?;
    let cache = out.join("cache");
    fs::create_dir_all(&cache)?;
    let mut encoded = Vec::new();
    for clip in &config.content.clips {
        let source = std::path::absolute(base.join(&clip.file))?;
        let input = vec!["-i".into(), source.clone().into_os_string()];
        let (dir, cached) = encode(&cache, input, Some(&source), &clip.video, &clip.audio)?;
        println!("    {}{}", clip.name, if cached { " (cached)" } else { "" });
        encoded.push(dir);
    }
    // Clip played under BD-J apps, after the declared ones: VLC only draws them over video.
    // 1 s of black, held on screen as a still: VLC 3 hangs if a playlist is stopped
    // mid-play, and a playlist that ends makes it hold back the app's graphics.
    let background = config.content.clips.len();
    let has_apps = !config.content.apps.is_empty();
    if has_apps {
        let input = [
            "-f",
            "lavfi",
            "-i",
            "color=black:size=1920x1080:rate=24000/1001:duration=1",
        ];
        let input = input.into_iter().map(OsString::from).collect();
        let (dir, _) = encode(&cache, input, None, &config::Video::default(), &[])?;
        encoded.push(dir);
    }
    // What this build didn't use, and encodes left half done.
    let used: HashSet<_> = encoded.iter().collect();
    for entry in fs::read_dir(&cache)? {
        let path = entry?.path();
        if !used.contains(&path) {
            remove(&path)?;
        }
    }

    println!("==> 6. Packaging clips with tsMuxeR...");
    let font = config.content.subtitle_font.as_ref().map(|f| base.join(f));
    for (i, clip) in config.content.clips.iter().enumerate() {
        let tracks = Tracks {
            audio: clip
                .audio
                .iter()
                .map(|a| config.track_language(&a.language))
                .collect(),
            subtitles: clip
                .subtitles
                .iter()
                .map(|s| (base.join(&s.file), config.track_language(&s.language)))
                .collect(),
            font: font.as_deref(),
        };
        let chapters = clip.chapter_times()?;
        mux_clip(
            i,
            &clip.video,
            &tracks,
            &chapters,
            &encoded[i],
            &media,
            &disc,
        )?;
    }
    if has_apps {
        let none = Tracks {
            audio: Vec::new(),
            subtitles: Vec::new(),
            font: None,
        };
        let video = config::Video::default();
        mux_clip(
            background,
            &video,
            &none,
            &[],
            &encoded[background],
            &media,
            &disc,
        )?;
    }
    write_playlists(&config, &disc)?;
    if has_apps {
        let file = disc.join(format!(
            "BDMV/PLAYLIST/{}.mpls",
            bdmv::file_name(background as u16)
        ));
        let mut data = fs::read(&file)?;
        mpls::set_infinite_still(&mut data);
        fs::write(&file, data)?;
    }

    println!("==> 7. Creating BACKUP...");
    let bdmv = disc.join("BDMV");
    for f in [
        "index.bdmv",
        "MovieObject.bdmv",
        "BDJO",
        "PLAYLIST",
        "CLIPINF",
    ] {
        copy(&bdmv.join(f), &bdmv.join("BACKUP").join(f))?;
    }
    let cert = disc.join("CERTIFICATE");
    for f in ["id.bdmv", "app.discroot.crt"] {
        if cert.join(f).exists() {
            copy(&cert.join(f), &cert.join("BACKUP").join(f))?;
        }
    }

    println!("==> 8. Generating UDF 2.50 image...");
    let iso = out.join(format!("{}.iso", config.disc.label));
    udf_image::write(&disc, &config.disc.label, time, &iso)
        .with_context(|| format!("writing {}", iso.display()))?;
    println!("{}", iso.display());
    Ok(())
}

/// Fails on a missing input file before any slow step: encoding can take hours.
fn check_inputs(config: &Config, base: &Path) -> Result<()> {
    let mut files = Vec::new();
    for clip in &config.content.clips {
        files.push((format!("clip {:?}", clip.name), &clip.file));
        for subtitle in &clip.subtitles {
            files.push((format!("clip {:?} subtitles", clip.name), &subtitle.file));
        }
    }
    for font in &config.content.fonts {
        files.push((format!("font {:?}", font.name), &font.file));
    }
    if let Some(font) = &config.content.subtitle_font {
        files.push(("content.subtitle_font".into(), font));
    }
    if let Some(thumbnail) = &config.disc.thumbnail {
        files.push(("disc.thumbnail".into(), thumbnail));
    }
    for (what, file) in files {
        if !base.join(file).is_file() {
            bail!("{what}: {} not found", base.join(file).display());
        }
    }
    // tsMuxeR gets the subtitle font and files under build/.
    quoted(&base.join("build"))?;
    if let Some(font) = &config.content.subtitle_font {
        quoted(&base.join(font))?;
    }
    for app in &config.content.apps {
        if java_files(&base.join(&app.src))?.is_empty() {
            bail!(
                "app {:?}: no .java files in {}",
                app.name,
                app.src.display()
            );
        }
    }
    Ok(())
}

/// Classes of app `n`.
fn app_classes(out: &Path, n: usize) -> PathBuf {
    out.join("apps")
        .join(bdmv::file_name(n as u16))
        .join("classes")
}

fn compile_apps(config: &Config, base: &Path, out: &Path) -> Result<()> {
    let api = out.join("bdj-api");
    fs::create_dir_all(&api)?;
    run(Command::new("jar")
        .current_dir(&api)
        .arg("xf")
        .arg(libbluray_jar()?)
        .args(BDJ_API))?;

    let titles = out.join("gen/bdbuild/Titles.java");
    fs::create_dir_all(titles.parent().unwrap())?;
    fs::write(&titles, titles_class(config)?)?;

    for (n, app) in config.content.apps.iter().enumerate() {
        let mut sources = java_files(&base.join(&app.src))?;
        sources.push(titles.clone());
        let classes = app_classes(out, n);
        fs::create_dir_all(&classes)?;
        run(Command::new("javac")
            .args(JAVAC_FLAGS)
            .arg("-d")
            .arg(classes)
            .arg("-cp")
            .arg(&api)
            .args(sources))?;
    }
    Ok(())
}

/// libbluray's BD-J jar (libbluray-j2se-<version>.jar).
fn libbluray_jar() -> Result<PathBuf> {
    if let Some(jar) = std::env::var_os("LIBBLURAY_JAR") {
        let jar = PathBuf::from(jar);
        if !jar.is_file() {
            bail!("LIBBLURAY_JAR: {} not found", jar.display());
        }
        return Ok(jar);
    }
    let mut jars: Vec<_> = fs::read_dir(LIBBLURAY_JAR_DIR)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(OsStr::to_str)
                .is_some_and(|n| n.starts_with("libbluray-j2se-") && n.ends_with(".jar"))
        })
        .collect();
    // Newest version: 1.10.0 after 1.9.0.
    jars.sort_by_key(|jar| {
        let name = jar.file_stem().and_then(OsStr::to_str).unwrap_or("");
        let version = name.trim_start_matches("libbluray-j2se-");
        version
            .split('.')
            .map(|n| n.parse::<u32>().unwrap_or(0))
            .collect::<Vec<_>>()
    });
    match jars.pop() {
        Some(jar) => Ok(jar),
        None => bail!(
            "libbluray-j2se-*.jar not found in {LIBBLURAY_JAR_DIR}: install libbluray or set LIBBLURAY_JAR"
        ),
    }
}

/// bdbuild.Titles: one constant per title with its number, so the app can jump to
/// titles by name ("piracy-warning" -> Titles.PIRACY_WARNING).
fn titles_class(config: &Config) -> Result<String> {
    let titles = config.navigation.titles.iter().enumerate();
    constants_class(
        "Titles",
        "",
        titles
            .map(|(i, t)| (format!("title {:?}", t.name), t.name.clone(), i + 1))
            .collect(),
    )
}

/// A bdbuild class with `header` and one int constant per (what, name, value), the name
/// turned into a Java constant name.
fn constants_class(
    class: &str,
    header: &str,
    constants: Vec<(String, String, usize)>,
) -> Result<String> {
    let mut java = format!(
        "// Generated by bd-build from the disc config.\npackage bdbuild;\n\npublic final class {class} {{\n{header}"
    );
    let mut names = Vec::new();
    for (what, name, value) in constants {
        let name: String = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_uppercase()
                } else {
                    '_'
                }
            })
            .collect();
        if !name.starts_with(|c: char| c.is_ascii_alphabetic()) {
            bail!("{what}: name must start with a letter");
        }
        if names.contains(&name) {
            bail!("{what}: {class}.{name} is already taken");
        }
        java += &format!("    public static final int {name} = {value};\n");
        names.push(name);
    }
    java += &format!("\n    private {class}() {{\n    }}\n}}\n");
    Ok(java)
}

/// Creates a keystore with a BD-J disc root certificate ("root") and an application
/// certificate signed by it ("app"), following the HD Cookbook BDCertGenerator:
/// RSA 1024, SHA1withRSA, validity until ~year 10000, DN attributes as UTF8String
/// (MHP 12.5.6), organisation id appended to O.
fn make_keys(keystore: &Path, signing: &config::Signing, org: &str, out: &Path) -> Result<()> {
    let (root_dn, app_dn) = cert_dns(signing, org);
    let email = format!("email:{}", signing.email);

    // The spec recommends an open-ended validity (BDCertGenerator uses year 0..9999,
    // both as GeneralizedTime). keytool can't start before 2000 (it maps 1949 to 2049),
    // so the start is UTCTime 2000-01-01; the end is past 2049, hence GeneralizedTime.
    let validity = ["-startdate", "2000/01/01 00:00:00", "-validity", "2900000"]; // ~7940 years
    let rsa = [
        "-keyalg",
        "RSA",
        "-keysize",
        "1024",
        "-sigalg",
        "SHA1withRSA",
    ];

    let kt = |args: &[&str]| -> Result<()> {
        run(Command::new("keytool")
            .arg("-keystore")
            .arg(keystore)
            .args([
                "-storepass",
                &signing.password,
                "-keypass",
                &signing.password,
            ])
            .args(args))
    };

    kt(&[
        &["-genkeypair", "-alias", "root", "-dname", &root_dn][..],
        &rsa,
        &validity,
        &[
            "-ext",
            "KU:critical=digitalSignature,keyCertSign",
            "-ext",
            "BC:critical=ca:true",
            "-ext",
            &format!("SAN={email}"),
            "-ext",
            &format!("IAN={email}"),
        ],
    ]
    .concat())?;
    kt(&[
        &["-genkeypair", "-alias", "app", "-dname", &app_dn][..],
        &rsa,
        &validity,
    ]
    .concat())?;

    let csr = out.join("app.csr");
    let crt = out.join("app.crt");
    let (csr, crt) = (csr.to_str().unwrap(), crt.to_str().unwrap());
    kt(&["-certreq", "-alias", "app", "-file", csr])?;
    kt(&[
        &[
            "-gencert",
            "-alias",
            "root",
            "-sigalg",
            "SHA1withRSA",
            "-infile",
            csr,
            "-outfile",
            crt,
            "-dname",
            &app_dn,
        ][..],
        &validity,
        &[
            "-ext",
            "KU:critical=digitalSignature",
            "-ext",
            &format!("SAN={email}"),
            "-ext",
            &format!("IAN={email}"),
        ],
    ]
    .concat())?;
    kt(&["-importcert", "-alias", "app", "-noprompt", "-file", crt])
}

/// Root and app certificate subjects: "<organization> Root" and "<organization> App",
/// with the org id appended to O.
fn cert_dns(signing: &config::Signing, org: &str) -> (String, String) {
    let dn = |role| {
        dn(
            &format!("{} {role}", signing.organization),
            &signing.unit,
            org,
            &signing.country,
        )
    };
    (dn("Root"), dn("App"))
}

/// Distinguished name with every attribute forced to UTF8String: "#" + DER (tag 0x0c, length, bytes).
fn dn(cn: &str, ou: &str, o: &str, c: &str) -> String {
    let utf8 = |s: &str| {
        let hex: String = s.bytes().map(|b| format!("{b:02x}")).collect();
        format!("#0c{:02x}{hex}", s.len())
    };
    format!(
        "CN={}, OU={}, O={}, C={}",
        utf8(cn),
        utf8(ou),
        utf8(o),
        utf8(c)
    )
}

fn sign_apps(
    config: &Config,
    signing: &config::Signing,
    base: &Path,
    out: &Path,
    disc: &Path,
    time: SystemTime,
) -> Result<()> {
    let keystore = base.join(&signing.keystore);
    let org = format!("{}.{}", signing.organization, config.disc.org_id);
    if keystore.exists() {
        // The root certificate must match the disc's org id, so a keystore made for other
        // settings can't be reused.
        let list = output(
            Command::new("keytool")
                .args([
                    "-J-Duser.language=en",
                    "-list",
                    "-v",
                    "-alias",
                    "root",
                    "-keystore",
                ]) // parsed below
                .arg(&keystore)
                .args(["-storepass", &signing.password]),
        )
        .with_context(|| {
            format!(
                "opening {} (is signing.password right?)",
                keystore.display()
            )
        })?;
        let owner = list
            .lines()
            .find_map(|l| l.strip_prefix("Owner: "))
            .unwrap_or("");
        let expected = format!(
            "CN={} Root, OU={}, O={org}, C={}",
            signing.organization, signing.unit, signing.country
        );
        if owner != expected {
            bail!(
                "keystore {} is for {owner:?}, not this config: delete it to create a new one",
                keystore.display()
            );
        }
    } else {
        println!("    creating {}", keystore.display());
        if let Some(dir) = keystore.parent() {
            fs::create_dir_all(dir)?;
        }
        make_keys(&keystore, signing, &org, out)?;
    }

    let tools = out.join("tools");
    fs::create_dir_all(&tools)?;
    fs::write(tools.join("BDJSign.java"), BDJ_SIGN)?;
    run(Command::new("javac")
        .args(["-XDignore.symbol.file", "-nowarn", "-d"])
        .arg(&tools)
        .arg(tools.join("BDJSign.java")))?;

    for n in 0..config.content.apps.len() {
        let jar = disc.join(format!("BDMV/JAR/{}.jar", bdmv::file_name(n as u16)));
        // Files in a fixed order: the manifest, and so the signature, list them in jar order.
        let classes = app_classes(out, n);
        let jar = std::path::absolute(&jar)?;
        run(Command::new("jar")
            .current_dir(&classes)
            .arg("cf")
            .arg(&jar)
            .args(files_under(&classes, Path::new(""))?))?;
        run(Command::new("jarsigner")
            .arg("-keystore")
            .arg(&keystore)
            .args([
                "-storepass",
                &signing.password,
                "-sigfile",
                "SIG-BD00",
                "-digestalg",
                "SHA1",
                "-sigalg",
                "SHA1withRSA",
            ])
            .arg(&jar)
            .arg("app"))?;
        let millis = time
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .to_string();
        run(Command::new("java")
            .arg("-cp")
            .arg(&tools)
            .arg("BDJSign")
            .arg(&jar)
            .arg(&keystore)
            .args([&signing.password, "app", &millis]))?;
    }
    run(Command::new("keytool")
        .args(["-exportcert", "-alias", "root", "-keystore"])
        .arg(&keystore)
        .args(["-storepass", &signing.password, "-file"])
        .arg(disc.join("CERTIFICATE/app.discroot.crt")))
}

/// What a title runs; BD-J titles add their BDJO to `bdjos`.
fn title_kind<'a>(
    config: &'a Config,
    t: &config::Title,
    bdjos: &mut Vec<bdmv::Bdjo<'a>>,
) -> Result<bdmv::TitleKind> {
    Ok(match &t.kind {
        config::TitleKind::App(title) => {
            let n = config.app_index(&title.name)?;
            let app = &config.content.apps[n as usize];
            bdjos.push(bdmv::Bdjo {
                app: n,
                name: &app.name,
                class: &app.class,
                havi_config_id: title.resolution.havi_config_id(),
            });
            bdmv::TitleKind::Bdj(bdjos.len() as u16 - 1)
        }
        config::TitleKind::Clip(clip) => bdmv::TitleKind::Play {
            playlist: config.clip_index(&clip.name)?,
            then: config.title_number(&clip.then)?,
            mask: config::uo_mask(&clip.mask),
            streams: streams(config, t)?,
        },
        config::TitleKind::Playlist(playlist) => bdmv::TitleKind::Play {
            playlist: config.playlist_number(&playlist.name)?,
            then: config.title_number(&playlist.then)?,
            mask: config::uo_mask(&playlist.mask),
            streams: streams(config, t)?,
        },
    })
}

/// The tracks a title starts with, from its `streams`.
fn streams(config: &Config, t: &config::Title) -> Result<Option<bdmv::Streams>> {
    Ok(config
        .stream_numbers(t)?
        .map(|(audio, subtitles)| bdmv::Streams { audio, subtitles }))
}

fn write_navigation(config: &Config, disc: &Path) -> Result<()> {
    let mut titles = Vec::new();
    let mut bdjos = Vec::new();
    for t in &config.navigation.titles {
        let kind =
            title_kind(config, t, &mut bdjos).with_context(|| format!("title {:?}", t.name))?;
        titles.push(bdmv::Title {
            hidden: t.hidden,
            kind,
        });
    }
    let nav = bdmv::Nav {
        titles,
        bdjos,
        first_play: config.title_number(&config.navigation.first_play)?,
        top_menu: config.title_number(&config.navigation.top_menu)?,
    };
    let meta = config.disc.name.as_deref().map(|name| bdmv::Meta {
        name,
        titles: config
            .navigation
            .titles
            .iter()
            .enumerate()
            .filter_map(|(i, t)| Some((i as u16 + 1, t.display_name.as_deref()?)))
            .collect(),
        thumbnail: config.disc.thumbnail.is_some(),
    });
    let files = bdmv::files(
        &nav,
        meta.as_ref(),
        &config.disc.language,
        config.content.clips.len() as u16, // the background clip, muxed after the declared ones
        config.org_id()?,
        &config.disc_id()?,
    );
    for (path, data) in files {
        let path = disc.join(path);
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write(path, data)?;
    }
    Ok(())
}

/// The declared playlists, made of the clips' own playlists written by tsMuxeR, and
/// the UO masks of the titles playing them.
fn write_playlists(config: &Config, disc: &Path) -> Result<()> {
    let mut masks = HashMap::new();
    for title in &config.navigation.titles {
        if let Some((playlist, mask)) = config.playback(title)? {
            masks.insert(playlist, mask);
        }
    }
    let dir = disc.join("BDMV/PLAYLIST");
    let mut clips = Vec::new();
    for i in 0..config.content.clips.len() {
        let file = dir.join(format!("{}.mpls", bdmv::file_name(i as u16)));
        let data = fs::read(&file).with_context(|| format!("reading {}", file.display()))?;
        clips.push(
            mpls::ClipPlaylist::parse(&data)
                .with_context(|| format!("parsing {}", file.display()))?,
        );
    }
    for playlist in &config.navigation.playlists {
        let mut items = Vec::new();
        for item in &playlist.items {
            items.push(&clips[config.clip_index(&item.clip)? as usize]);
        }
        let chapters = playlist.chapters();
        let n = config.playlist_number(&playlist.name)?;
        let mask = masks.get(&n).copied().unwrap_or(0);
        fs::write(
            dir.join(format!("{}.mpls", bdmv::file_name(n))),
            mpls::playlist(&items, &chapters, mask),
        )?;
    }
    // After building the playlists above from them: items don't take a playlist's mask.
    for i in 0..config.content.clips.len() as u16 {
        if let Some(&mask) = masks.get(&i) {
            let file = dir.join(format!("{}.mpls", bdmv::file_name(i)));
            let mut data = fs::read(&file)?;
            mpls::set_uo_mask(&mut data, mask);
            fs::write(&file, data)?;
        }
    }
    Ok(())
}

fn write_fonts(config: &Config, base: &Path, disc: &Path) -> Result<()> {
    let auxdata = disc.join("BDMV/AUXDATA");
    let mut index = Vec::new();
    for (i, font) in config.content.fonts.iter().enumerate() {
        let file = format!("{i:05}.otf");
        copy(&base.join(&font.file), &auxdata.join(&file))?;
        index.push((font.name.as_str(), font.style.index_name(), file));
    }
    if !index.is_empty() {
        fs::write(auxdata.join("dvb.fontindex"), bdmv::font_index(&index))?;
    }
    Ok(())
}

/// Encodes the first video stream of `input` (ffmpeg input arguments) to video.h264,
/// and the source audio stream of each track to audio.<track>.ac3, in a directory of
/// `cache` named after everything that makes them: the arguments, and the size and
/// modification time of the input `file`. Returns it, and whether it was already there.
fn encode(
    cache: &Path,
    input: Vec<OsString>,
    file: Option<&Path>,
    video: &config::Video,
    audio: &[config::Audio],
) -> Result<(PathBuf, bool)> {
    let (w, h) = video.size();
    let rate = video.rate()?;
    let mut args = input;
    let mut add = |list: &[&str]| args.extend(list.iter().map(OsString::from));
    add(&["-map", "0:v:0", "-r", rate.ffmpeg, "-vf"]);
    add(&[&format!(
        "scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2"
    )]);
    add(&[
        "-c:v",
        "libx264",
        "-preset",
        &video.preset,
        "-crf",
        &video.crf.to_string(),
    ]);
    add(&[
        "-profile:v",
        "high",
        "-level",
        "4.1",
        "-pix_fmt",
        "yuv420p",
        "-x264-params",
    ]);
    add(&[&format!(
        "bluray-compat=1:vbv-maxrate=40000:vbv-bufsize=30000:keyint={}:min-keyint=1:slices=4:\
         nal-hrd=vbr:b-pyramid=strict:open-gop=0",
        rate.keyint
    )]);
    add(&["video.h264"]);
    for (i, track) in audio.iter().enumerate() {
        let stream = format!("0:a:{}", track.stream.unwrap_or(i));
        let channels = track.channels.to_string();
        add(&[
            "-map",
            &stream,
            "-c:a",
            "ac3",
            "-b:a",
            &track.bitrate,
            "-ac",
            &channels,
        ]);
        add(&[&format!("audio.{i}.ac3")]);
    }

    let mut key = Fnv::default();
    for arg in &args {
        key.write(arg.as_encoded_bytes());
        key.write(&[0]);
    }
    if let Some(file) = file {
        let meta = fs::metadata(file)?;
        let modified = meta
            .modified()?
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        key.write(&meta.len().to_le_bytes());
        key.write(&modified.as_nanos().to_le_bytes());
    }
    let dir = cache.join(format!("{:016x}", key.0));
    if dir.is_dir() {
        return Ok((dir, true));
    }
    // Written aside and renamed when done, so a failed encode is never taken as done.
    let partial = cache.join(format!("{:016x}.partial", key.0));
    if partial.exists() {
        remove(&partial)?;
    }
    fs::create_dir_all(&partial)?;
    run(Command::new("ffmpeg")
        .current_dir(&partial)
        .args(["-v", "error", "-y"])
        .args(&args))?;
    fs::rename(&partial, &dir)?;
    Ok((dir, false))
}

/// FNV-1a: a stable 64-bit hash for cache keys.
struct Fnv(u64);

impl Default for Fnv {
    fn default() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
}

impl Fnv {
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0 ^ b as u64).wrapping_mul(0x0100_0000_01b3);
        }
    }
}

/// The disc thumbnail: `image` as a 640x360 JPEG in BDMV/META/DL.
fn write_thumbnail(image: &Path, disc: &Path) -> Result<()> {
    let dir = disc.join("BDMV/META/DL");
    fs::create_dir_all(&dir)?;
    run(Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-i"])
        .arg(image)
        .args(["-frames:v", "1", "-vf"])
        .arg("scale=640:360:force_original_aspect_ratio=decrease,pad=640:360:(ow-iw)/2:(oh-ih)/2")
        .arg(dir.join(bdmv::THUMBNAIL)))
}

/// A clip's tracks besides video: audio languages, subtitle files and languages, and the
/// font subtitles are drawn with.
struct Tracks<'a> {
    audio: Vec<&'a str>,
    subtitles: Vec<(PathBuf, &'a str)>,
    font: Option<&'a Path>,
}

/// Muxes clip `number` from its encoded streams with tsMuxeR, into its m2ts, clip info
/// and single-clip playlist.
#[allow(clippy::too_many_arguments)]
fn mux_clip(
    number: usize,
    video: &config::Video,
    tracks: &Tracks,
    chapters: &[u64],
    encoded: &Path,
    media: &Path,
    disc: &Path,
) -> Result<()> {
    let name = bdmv::file_name(number as u16);
    // Chapters as playlist marks; tsMuxeR's own is a single one at the start.
    let chapters = if chapters.is_empty() {
        String::new()
    } else {
        let times: Vec<String> = std::iter::once(0)
            .chain(chapters.iter().copied())
            .map(|ms| {
                format!(
                    "{:02}:{:02}:{:02}.{:03}",
                    ms / 3_600_000,
                    ms / 60_000 % 60,
                    ms / 1000 % 60,
                    ms % 1000
                )
            })
            .collect();
        format!(" --custom-chapters={}", times.join(";"))
    };
    let mut meta = format!(
        "MUXOPT --no-pcr-on-video-pid --new-audio-pes --blu-ray --vbr --vbv-len=500 \
         --mplsOffset={number} --m2tsOffset={number}{chapters}\n\
         V_MPEG4/ISO/AVC, {}, fps={}, insertSEI, contSPS\n",
        quoted(&encoded.join("video.h264"))?,
        video.rate()?.tsmuxer
    );
    for (i, language) in tracks.audio.iter().enumerate() {
        meta += &format!(
            "A_AC3, {}, lang={language}\n",
            quoted(&encoded.join(format!("audio.{i}.ac3")))?
        );
    }
    // SRT subtitles, turned into PGS images by tsMuxeR. It drops the last entry of a file
    // that doesn't end in a blank line, so it gets a copy that does.
    let (w, h) = video.size();
    for (i, (file, language)) in tracks.subtitles.iter().enumerate() {
        let srt =
            fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
        let file = media.join(format!("{name}.{i}.srt"));
        fs::write(&file, srt.trim_end().to_owned() + "\n\n")?;
        meta += &format!(
            "S_TEXT/UTF8, {}, font-name={}, font-size={}, font-color=0xffffffff, bottom-offset={}, \
             font-border=2, text-align=center, video-width={w}, video-height={h}, fps={}, lang={language}\n",
            quoted(&file)?,
            quoted(tracks.font.unwrap())?,
            h * 65 / 1080,
            h * 24 / 1080,
            video.rate()?.tsmuxer,
        );
    }
    let meta_file = media.join(format!("{name}.meta"));
    fs::write(&meta_file, meta)?;
    let bd = media.join(format!("bd{number}"));
    run(Command::new("tsmuxer").arg(&meta_file).arg(&bd))?;
    for d in ["STREAM", "CLIPINF", "PLAYLIST"] {
        for entry in fs::read_dir(bd.join("BDMV").join(d))? {
            let entry = entry?;
            copy(
                &entry.path(),
                &disc.join("BDMV").join(d).join(entry.file_name()),
            )?;
        }
    }
    Ok(())
}

/// A path in quotes, as tsMuxeR's .meta files take them (with no escape for quotes).
fn quoted(path: &Path) -> Result<String> {
    let path = path.display().to_string();
    if path.contains('"') {
        bail!("{path}: tsMuxeR can't take paths with quotes");
    }
    Ok(format!("\"{path}\""))
}

/// Runs a command, showing its output only if it fails.
fn run(cmd: &mut Command) -> Result<()> {
    output(cmd).map(|_| ())
}

/// Runs a command and returns its stdout, showing its output only if it fails.
fn output(cmd: &mut Command) -> Result<String> {
    let name = cmd.get_program().to_string_lossy().into_owned();
    let output = cmd.output().with_context(|| format!("running {name}"))?;
    if !output.status.success() {
        eprint!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        bail!("{name} failed ({})", output.status);
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Files under `dir`, recursively and sorted, as paths relative to it (prefixed by `rel`).
fn files_under(dir: &Path, rel: &Path) -> Result<Vec<PathBuf>> {
    let mut names: Vec<_> = fs::read_dir(dir)?
        .map(|e| e.map(|e| e.file_name()))
        .collect::<Result<_, _>>()?;
    names.sort();
    let mut files = Vec::new();
    for name in names {
        if dir.join(&name).is_dir() {
            files.extend(files_under(&dir.join(&name), &rel.join(&name))?);
        } else {
            files.push(rel.join(name));
        }
    }
    Ok(files)
}

/// .java files under a directory, recursively.
fn java_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let path = entry?.path();
        if path.is_dir() {
            files.extend(java_files(&path)?);
        } else if path.extension() == Some(OsStr::new("java")) {
            files.push(path);
        }
    }
    Ok(files)
}

/// Removes a file, or a directory with its contents.
fn remove(path: &Path) -> Result<()> {
    if path.is_dir() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

/// Copies a file, or a directory of files.
fn copy(from: &Path, to: &Path) -> Result<()> {
    if from.is_dir() {
        fs::create_dir_all(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            copy(&entry.path(), &to.join(entry.file_name()))?;
        }
    } else {
        fs::copy(from, to).with_context(|| format!("copying {}", from.display()))?;
    }
    Ok(())
}
