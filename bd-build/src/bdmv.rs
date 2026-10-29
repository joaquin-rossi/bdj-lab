//! Binary navigation files for a BD-J disc:
//!
//!   BDMV/index.bdmv        first play, top menu and titles; each is either a BD-J
//!                          object or an HDMV movie object
//!   BDMV/MovieObject.bdmv  one object per playing title (SetStream if it picks its tracks,
//!                          PlayPL, JumpTitle), plus a
//!                          JumpTitle object for a first play / top menu that targets an
//!                          HDMV title, so it plays as a numbered title and players know
//!                          its duration
//!   BDMV/BDJO/NNNNN.bdjo   one per BD-J title: an autostart Xlet loaded from an app's
//!                          BDMV/JAR/NNNNN.jar, over a background playlist
//!   BDMV/META/DL/bdmt_LLL.xml  disc name, title names and thumbnail shown by players,
//!                              LLL being their language
//!   BDMV/AUXDATA/dvb.fontindex  font names for org.dvb.ui.FontFactory
//!   CERTIFICATE/id.bdmv    organisation id + disc id (the org id must match the signing
//!                          certificate)
//!
//! Layouts follow the BD-ROM spec as parsed by libbluray
//! (src/libbluray/bdnav/index_parse.c, hdmv/mobj_parse.c, bdj/bdjo_parse.c).

const VERSION: &[u8] = b"0200";
const APP_ID: u16 = 0x4000; // first id of the signed-app range; unsigned apps are fine on libbluray

/// 5-digit file name: BDMV/JAR/NNNNN.jar, BDMV/BDJO/NNNNN.bdjo.
pub fn file_name(n: u16) -> String {
    format!("{n:05}")
}

/// What a BD-J title runs.
pub struct Bdjo<'a> {
    /// App index, which is also its JAR number.
    pub app: u16,
    pub name: &'a str,
    pub class: &'a str,
    pub havi_config_id: u8,
}

pub struct Title {
    pub hidden: bool,
    pub kind: TitleKind,
}

pub enum TitleKind {
    /// Run a BD-J object (index into the BDJO list).
    Bdj(u16),
    /// Play a playlist, then jump to a title (numbered from 1). `mask` is the UO mask
    /// of the playlist, whose menu and title search bits also go in the movie object.
    Play {
        playlist: u16,
        then: u16,
        mask: u64,
        streams: Option<Streams>,
    },
}

/// Tracks a playing title starts with, as numbers from 1: audio (None: the player's
/// choice) and subtitles (None: off).
#[derive(Clone, Copy)]
pub struct Streams {
    pub audio: Option<u16>,
    pub subtitles: Option<u16>,
}

pub struct Nav<'a> {
    pub titles: Vec<Title>,
    pub bdjos: Vec<Bdjo<'a>>,
    /// Title numbers.
    pub first_play: u16,
    pub top_menu: u16,
}

#[derive(Default)]
struct Buf(Vec<u8>);

impl Buf {
    fn u8(mut self, v: u8) -> Self {
        self.0.push(v);
        self
    }
    fn u16(mut self, v: u16) -> Self {
        self.0.extend(v.to_be_bytes());
        self
    }
    fn u32(mut self, v: u32) -> Self {
        self.0.extend(v.to_be_bytes());
        self
    }
    fn bytes(mut self, b: &[u8]) -> Self {
        self.0.extend(b);
        self
    }
    fn zeros(self, n: usize) -> Self {
        self.bytes(&vec![0; n])
    }
    fn with_len32(self, body: Buf) -> Self {
        self.u32(body.0.len() as u32).bytes(&body.0)
    }
    fn len(&self) -> usize {
        self.0.len()
    }
}

// ---------------------------------------------------------------- index.bdmv

/// access_type bits: title number not shown in the player UI (jumps still allowed).
const ACCESS_HIDDEN: u32 = 0b10;

/// object_type(2)=2 BD-J, access_type(2), reserved(28) | playback_type(2)=3 interactive,
/// reserved(14), name(5), reserved(8)
fn bdj_object(b: Buf, access: u32, bdjo: u16) -> Buf {
    b.u32((2 << 30) | (access << 28))
        .u16(3 << 14)
        .bytes(file_name(bdjo).as_bytes())
        .u8(0)
}

/// object_type(2)=1 HDMV, access_type(2), reserved(28) | playback_type(2)=0 movie,
/// reserved(14), id_ref(16), reserved(32)
fn hdmv_object(b: Buf, access: u32, id_ref: u16) -> Buf {
    b.u32((1 << 30) | (access << 28)).u16(0).u16(id_ref).u32(0)
}

#[derive(Clone, Copy)]
enum Object {
    /// BD-J object (BDJO index).
    Bdj(u16),
    /// HDMV movie object id.
    Hdmv(u16),
}

/// HDMV movie objects, as mask flags and commands, and the object used by each title,
/// first play and top menu.
struct MovieObjects {
    objects: Vec<(u16, Vec<Buf>)>,
    titles: Vec<Object>,
    first_play: Object,
    top_menu: Object,
}

fn movie_objects(nav: &Nav) -> MovieObjects {
    let mut objects = Vec::new();
    let mut add = |flags: u16, cmds: Vec<Buf>| {
        objects.push((flags, cmds));
        Object::Hdmv(objects.len() as u16 - 1)
    };
    let titles = nav
        .titles
        .iter()
        .map(|t| match t.kind {
            TitleKind::Bdj(bdjo) => Object::Bdj(bdjo),
            TitleKind::Play {
                playlist,
                then,
                mask,
                streams,
            } => {
                let mut cmds: Vec<Buf> = streams.map(set_stream).into_iter().collect();
                cmds.extend([play_pl(playlist), jump_title(then)]);
                add(mask_flags(mask), cmds)
            }
        })
        .collect::<Vec<_>>();
    let mut entry = |title: u16| match nav.titles[title as usize - 1].kind {
        TitleKind::Bdj(bdjo) => Object::Bdj(bdjo),
        TitleKind::Play { .. } => add(0, vec![jump_title(title)]),
    };
    let first_play = entry(nav.first_play);
    let top_menu = entry(nav.top_menu);
    MovieObjects {
        objects,
        titles,
        first_play,
        top_menu,
    }
}

/// Movie object flags: resume intention(1), menu call mask(1), title search mask(1),
/// reserved(13), taken from the first two bits of a UO mask table.
fn mask_flags(uo_mask: u64) -> u16 {
    ((uo_mask >> 63 & 1) << 14 | (uo_mask >> 62 & 1) << 13) as u16
}

fn index_entry(b: Buf, access: u32, object: Object) -> Buf {
    match object {
        Object::Bdj(bdjo) => bdj_object(b, access, bdjo),
        Object::Hdmv(id) => hdmv_object(b, access, id),
    }
}

fn index_bdmv(nav: &Nav, mobj: &MovieObjects) -> Vec<u8> {
    let app_info = Buf::default().with_len32(
        Buf::default()
            .u8(0) // reserved / output-mode flags
            .u8(0) // video_format(4), frame_rate(4): unspecified
            .zeros(32), // user data
    );
    let mut indexes = index_entry(Buf::default(), 0, mobj.first_play);
    indexes = index_entry(indexes, 0, mobj.top_menu).u16(mobj.titles.len() as u16);
    for (title, &object) in nav.titles.iter().zip(&mobj.titles) {
        indexes = index_entry(
            indexes,
            if title.hidden { ACCESS_HIDDEN } else { 0 },
            object,
        );
    }
    let index_start = 40 + app_info.len() as u32;
    let header = Buf::default()
        .bytes(b"INDX")
        .bytes(VERSION)
        .u32(index_start)
        .u32(0)
        .zeros(24);
    assert_eq!(header.len(), 40);
    header.bytes(&app_info.0).with_len32(indexes).0
}

// ----------------------------------------------------------- MovieObject.bdmv

const GRP_BRANCH: u8 = 0;
const GRP_SET: u8 = 2;

/// Command with one or two immediate operands (hdmv/mobj_parse.c, hdmv/hdmv_insn.h):
/// op_cnt(3) grp(2) sub_grp(3) | imm_op1(1) imm_op2(1) reserved(2) branch_opt(4)
/// | reserved(4) cmp_opt(4) | reserved(3) set_opt(5) | dst(32) | src(32)
fn hdmv_cmd(grp: u8, sub_grp: u8, branch_opt: u8, set_opt: u8, dst: u32, src: Option<u32>) -> Buf {
    let op_cnt = 1 + src.is_some() as u8;
    Buf::default()
        .u8((op_cnt << 5) | (grp << 3) | sub_grp)
        .u8(0x80 | (src.is_some() as u8) << 6 | branch_opt)
        .u8(0)
        .u8(set_opt)
        .u32(dst)
        .u32(src.unwrap_or(0))
}

fn play_pl(playlist: u16) -> Buf {
    hdmv_cmd(GRP_BRANCH, 2, 0, 0, playlist.into(), None) // PLAY sub-group, PlayPL
}

fn jump_title(title: u16) -> Buf {
    hdmv_cmd(GRP_BRANCH, 1, 1, 0, title.into(), None) // JUMP sub-group, JumpTitle
}

/// SetStream (SETSYSTEM sub-group, option 1), as libbluray's _set_stream reads it:
/// audio flag(1), reserved(3), audio track(12) | PG flag(1), display flag(1), reserved(2),
/// PG track(12). The display flag is always applied: subtitles not selected are hidden.
fn set_stream(streams: Streams) -> Buf {
    let mut dst = 0u32;
    if let Some(audio) = streams.audio {
        dst |= 1 << 31 | (audio as u32) << 16;
    }
    if let Some(subtitles) = streams.subtitles {
        dst |= 1 << 15 | 1 << 14 | subtitles as u32;
    }
    hdmv_cmd(GRP_SET, 1, 0, 1, dst, Some(0))
}

fn movieobject_bdmv(mobj: &MovieObjects) -> Vec<u8> {
    // reserved, number_of_mobjs, then each object: flags, number of commands, commands
    let mut data = Buf::default().u32(0).u16(mobj.objects.len() as u16);
    for (flags, cmds) in &mobj.objects {
        data = data.u16(*flags).u16(cmds.len() as u16);
        for c in cmds {
            data = data.bytes(&c.0);
        }
    }
    let header = Buf::default()
        .bytes(b"MOBJ")
        .bytes(VERSION)
        .u32(0)
        .zeros(28);
    header.with_len32(data).0
}

// ------------------------------------------------------------- NNNNN.bdjo

/// Length byte + bytes, padded so the field occupies an even number of bytes.
fn app_string(b: Buf, s: &str) -> Buf {
    let b = b.u8(s.len() as u8).bytes(s.as_bytes());
    if s.len().is_multiple_of(2) {
        b.u8(0)
    } else {
        b
    }
}

fn bdjo(bdjo: &Bdjo, language: &str, org_id: u32, background_playlist: u16) -> Vec<u8> {
    let jar = file_name(bdjo.app);
    let terminal_info = Buf::default().with_len32(
        Buf::default()
            .bytes(b"*****") // default font: none on disc
            .u8(bdjo.havi_config_id << 4) // initial HAVi config(4), menu/title-search masks(2), pad
            .zeros(4),
    );

    let app_cache_info = Buf::default().with_len32(
        Buf::default()
            .u8(1) // one item
            .u8(0) // reserved
            .u8(1) // type 1 = JAR
            .bytes(jar.as_bytes())
            .bytes(b"*.*") // any language
            .zeros(3),
    );

    let accessible_playlists = Buf::default().with_len32(
        Buf::default()
            .u32((1 << 21) | (1 << 20) | (1 << 19)) // num_pl=1, access_to_all=1, autostart_first=1
            .bytes(format!("{background_playlist:05}").as_bytes())
            .u8(0),
    );

    let names = Buf::default()
        .bytes(language.as_bytes())
        .u8(bdjo.name.len() as u8)
        .bytes(bdjo.name.as_bytes());
    let names_pad = names.len() % 2;
    let mut descriptor = Buf::default()
        .u16(1 << 12) // one profile
        .u16(1) // profile 1
        .u8(1) // version 1.0.0
        .u8(0)
        .u8(0)
        .u8(0)
        .u8(1) // priority
        .u8((3 << 6) | (3 << 4)) // binding: title+disc bound, visibility: visible
        .u16(names.len() as u16)
        .bytes(&names.0)
        .zeros(names_pad);
    descriptor = app_string(descriptor, ""); // icon locator
    descriptor = descriptor.u16(0); // icon flags
    descriptor = app_string(descriptor, &jar); // base directory -> NNNNN.jar
    descriptor = app_string(descriptor, ""); // classpath extension
    descriptor = app_string(descriptor, bdjo.class);
    descriptor = app_string(descriptor, ""); // parameters (none)

    let app = Buf::default()
        .u8(1) // control code: autostart
        .u8(1 << 4) // application type 1 = BD-J
        .u32(org_id)
        .u16(APP_ID + bdjo.app)
        .u8(0) // descriptor tag
        .u8(0)
        .u32(descriptor.len() as u32)
        .u32(0)
        .bytes(&descriptor.0);
    let app_table = Buf::default().with_len32(Buf::default().u8(1).u8(0).bytes(&app.0));

    let key_interest = Buf::default().u32(0);
    let file_access = Buf::default().u16(1).bytes(b".");

    let sections = [
        terminal_info,
        app_cache_info,
        accessible_playlists,
        app_table,
        key_interest,
        file_access,
    ];
    const HEADER_LEN: usize = 8 + 40;
    let mut offsets = Vec::new();
    let mut pos = HEADER_LEN;
    for s in &sections {
        offsets.push(pos as u32);
        pos += s.len();
    }

    // address table: starts of app cache, playlists, app table, key interest,
    // file access, extension data (none) + 16 reserved bytes
    let mut out = Buf::default().bytes(b"BDJO").bytes(VERSION);
    for &o in &offsets[1..] {
        out = out.u32(o);
    }
    out = out.u32(0).zeros(16);
    assert_eq!(out.len(), HEADER_LEN);
    for s in &sections {
        out = out.bytes(&s.0);
    }
    out.0
}

// ------------------------------------------------------- CERTIFICATE/id.bdmv

fn id_bdmv(org_id: u32, disc_id: &[u8; 16]) -> Vec<u8> {
    let header = Buf::default()
        .bytes(b"BDID")
        .bytes(VERSION)
        .u32(40)
        .u32(0)
        .zeros(24);
    header.u32(org_id).bytes(disc_id).0
}

// --------------------------------------------------- AUXDATA/dvb.fontindex

/// (name, style, file name in AUXDATA)
pub fn font_index(fonts: &[(&str, &str, String)]) -> Vec<u8> {
    let mut xml = String::from(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE fontdirectory PUBLIC "-//DVB//DTD Font Directory 1.0//EN" "http://www.dvb.org/mhp/dtd/fontdirectory-1-0.dtd">
<fontdirectory>
"#,
    );
    for (name, style, file) in fonts {
        xml += &format!(
            "  <font>\n    <name>{name}</name>\n    <fontformat>OTF</fontformat>\n    \
             <filename>{file}</filename>\n    <style>{style}</style>\n    \
             <size min=\"0\" max=\"65535\"/>\n  </font>\n"
        );
    }
    xml += "</fontdirectory>\n";
    xml.into_bytes()
}

// ------------------------------------------------- META/DL/bdmt_LLL.xml

/// The disc thumbnail's file name in BDMV/META/DL, and its size.
pub const THUMBNAIL: &str = "thumb.jpg";
const THUMBNAIL_SIZE: &str = "640x360";

/// What players show about the disc.
pub struct Meta<'a> {
    pub name: &'a str,
    /// (title number, name)
    pub titles: Vec<(u16, &'a str)>,
    /// Whether BDMV/META/DL/THUMBNAIL is there.
    pub thumbnail: bool,
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Disc library metadata (libbluray bdnav/meta_parse.c).
fn disc_library(meta: &Meta, language: &str) -> Vec<u8> {
    let mut description = String::new();
    if !meta.titles.is_empty() {
        description += "      <di:tableOfContents>\n";
        for (number, name) in &meta.titles {
            description += &format!(
                "        <di:titleName titleNumber=\"{number}\">{}</di:titleName>\n",
                xml_escape(name)
            );
        }
        description += "      </di:tableOfContents>\n";
    }
    if meta.thumbnail {
        description +=
            &format!("      <di:thumbnail href=\"{THUMBNAIL}\" size=\"{THUMBNAIL_SIZE}\"/>\n");
    }
    if !description.is_empty() {
        description = format!("    <di:description>\n{description}    </di:description>\n");
    }
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<disclib xmlns="urn:BDA:bdmv;disclib" xmlns:di="urn:BDA:bdmv;discinfo">
  <di:discinfo>
    <di:title>
      <di:name>{}</di:name>
    </di:title>
{description}    <di:language>{language}</di:language>
  </di:discinfo>
</disclib>
"#,
        xml_escape(meta.name)
    )
    .into_bytes()
}

/// All navigation files, as (path relative to the disc root, contents).
pub fn files(
    nav: &Nav,
    meta: Option<&Meta>,
    language: &str,
    background_playlist: u16,
    org_id: u32,
    disc_id: &[u8; 16],
) -> Vec<(String, Vec<u8>)> {
    let mobj = movie_objects(nav);
    let mut files = vec![
        ("BDMV/index.bdmv".into(), index_bdmv(nav, &mobj)),
        ("BDMV/MovieObject.bdmv".into(), movieobject_bdmv(&mobj)),
        ("CERTIFICATE/id.bdmv".into(), id_bdmv(org_id, disc_id)),
    ];
    for (n, b) in nav.bdjos.iter().enumerate() {
        files.push((
            format!("BDMV/BDJO/{}.bdjo", file_name(n as u16)),
            bdjo(b, language, org_id, background_playlist),
        ));
    }
    if let Some(meta) = meta {
        files.push((
            format!("BDMV/META/DL/bdmt_{language}.xml"),
            disc_library(meta, language),
        ));
    }
    files
}
