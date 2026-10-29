//! Writes a directory as a UDF 2.50 image laid out like a Blu-ray disc: read-only, file
//! data in a physical partition, and the file set, directories and file entries in a
//! metadata partition with a mirror copy.
//!
//! Image layout, in 2048-byte sectors:
//!
//!   16-18     volume recognition sequence (BEA01, NSR03, TEA01)
//!   32, 48    main and reserve volume descriptor sequences
//!   64        logical volume integrity descriptor
//!   256, last anchor volume descriptor pointers
//!   257-      physical partition: metadata file entry, its mirror, the metadata
//!             partition contents twice, then file data
//!
//! Layouts follow ECMA-167 3rd edition and OSTA UDF 2.50, as read by libudfread (which
//! libbluray and VLC open images with) and the Linux udf driver.

use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const BLOCK: u32 = 2048;
const REVISION: u16 = 0x0250;
const MAIN_VDS: u32 = 32;
const RESERVE_VDS: u32 = 48;
const VDS_SECTORS: u32 = 16;
const INTEGRITY: u32 = 64;
const ANCHOR: u32 = 256;
const PARTITION_START: u32 = 257;
/// Allocation and alignment unit of the metadata partition, in blocks.
const META_UNIT: u32 = 32;
/// Longest extent a descriptor records (30-bit length) that is a whole number of blocks.
const MAX_EXTENT: u64 = (1 << 30) - BLOCK as u64;

/// Partition reference numbers (indexes into the logical volume's partition maps).
const PHYSICAL: u16 = 0;
const METADATA: u16 = 1;

// Descriptor tag identifiers (ECMA-167 3/7.2.1, 4/7.2.1).
const TAG_PVD: u16 = 1;
const TAG_AVDP: u16 = 2;
const TAG_IUVD: u16 = 4;
const TAG_PD: u16 = 5;
const TAG_LVD: u16 = 6;
const TAG_USD: u16 = 7;
const TAG_TD: u16 = 8;
const TAG_LVID: u16 = 9;
const TAG_FSD: u16 = 256;
const TAG_FID: u16 = 257;
const TAG_EFE: u16 = 266;

// File types (ECMA-167 4/14.6.6, UDF 2.50 2.3.5.2).
const FILE_DIR: u8 = 4;
const FILE_DATA: u8 = 5;
const FILE_METADATA: u8 = 250;
const FILE_METADATA_MIRROR: u8 = 251;

/// Writes the files under `root` to a UDF 2.50 image at `out`, with `volume_id` (ASCII,
/// up to 30 characters) as volume, logical volume and file set identifier, and `time` as
/// every recording, creation and modification time (the same inputs give the same image).
pub fn write(root: &Path, volume_id: &str, time: SystemTime, out: &Path) -> io::Result<()> {
    if volume_id.is_empty() || volume_id.len() > 30 || !volume_id.is_ascii() {
        return Err(invalid(format!(
            "volume id {volume_id:?}: must be 1 to 30 ASCII characters"
        )));
    }
    let mut entries = vec![Entry {
        name: String::new(),
        parent: 0,
        kind: Kind::Dir(Vec::new()),
        unique_id: 0,
        fe: 0,
        data: 0,
    }];
    scan(root, 0, &mut entries)?;
    let layout = Layout::new(&mut entries);
    let time = Time::new(time, volume_id);

    let mut img = File::create(out)?;
    img.set_len(layout.sectors as u64 * BLOCK as u64)?;
    let mut put = |sector: u32, data: &[u8]| -> io::Result<()> {
        img.seek(SeekFrom::Start(sector as u64 * BLOCK as u64))?;
        img.write_all(data)
    };

    // Volume recognition sequence (ECMA-167 2/9.1, 3/9.1).
    for (i, id) in [b"BEA01", b"NSR03", b"TEA01"].into_iter().enumerate() {
        let mut d = vec![0; BLOCK as usize];
        d[1..6].copy_from_slice(id);
        d[6] = 1;
        put(16 + i as u32, &d)?;
    }

    for start in [MAIN_VDS, RESERVE_VDS] {
        for (i, d) in volume_descriptors(start, volume_id, &layout, &time)
            .into_iter()
            .enumerate()
        {
            put(start + i as u32, &d)?;
        }
    }
    put(INTEGRITY, &integrity(&entries, &layout, &time))?;
    put(INTEGRITY + 1, &terminator(INTEGRITY + 1))?;
    for sector in [ANCHOR, layout.sectors - 1] {
        put(sector, &anchor(sector))?;
    }

    // Metadata file and mirror: file entries in the physical partition whose single
    // extent holds the metadata partition.
    for (block, file_type, extent) in [
        (0, FILE_METADATA, layout.meta_start),
        (1, FILE_METADATA_MIRROR, layout.mirror_start),
    ] {
        let ads = Ads::Short(vec![(layout.meta_blocks as u64 * BLOCK as u64, extent)]);
        let fe = efe(
            file_type,
            layout.meta_blocks as u64 * BLOCK as u64,
            &ads,
            0,
            1,
            0,
            &time,
            block,
        )?;
        put(PARTITION_START + block, &fe)?;
    }

    // Metadata partition contents, written at both extents.
    let mut meta = vec![
        (0, file_set(volume_id, &entries, &time)),
        (1, terminator(1)),
    ];
    for (i, e) in entries.iter().enumerate() {
        match &e.kind {
            Kind::Dir(children) => {
                let stream = directory(&entries, i, children);
                let ads = Ads::Short(vec![(stream.len() as u64, e.data)]);
                let links = 1 + children
                    .iter()
                    .filter(|&&c| matches!(entries[c].kind, Kind::Dir(_)))
                    .count() as u16;
                meta.push((
                    e.fe,
                    efe(
                        FILE_DIR,
                        stream.len() as u64,
                        &ads,
                        e.unique_id,
                        links,
                        0x14a5,
                        &time,
                        e.fe,
                    )?,
                ));
                meta.push((e.data, stream));
            }
            Kind::File { len, .. } => {
                let ads = Ads::Long(extents(*len, e.data));
                meta.push((
                    e.fe,
                    efe(FILE_DATA, *len, &ads, e.unique_id, 1, 0x1084, &time, e.fe)?,
                ));
            }
        }
    }
    for start in [layout.meta_start, layout.mirror_start] {
        for (block, data) in &meta {
            put(PARTITION_START + start + block, data)?;
        }
    }

    for e in &entries {
        if let Kind::File { path, len } = &e.kind {
            img.seek(SeekFrom::Start(
                (PARTITION_START + e.data) as u64 * BLOCK as u64,
            ))?;
            // Exactly the recorded length: a file that changed since the scan would
            // otherwise spill into the next one's blocks.
            let mut file = File::open(path)?;
            let copied = io::copy(&mut (&mut file).take(*len), &mut img)?;
            if copied != *len || file.read(&mut [0])? != 0 {
                return Err(invalid(format!(
                    "{}: changed while writing the image",
                    path.display()
                )));
            }
        }
    }
    img.sync_all()
}

// ------------------------------------------------------------------- file tree

struct Entry {
    name: String,
    parent: usize,
    kind: Kind,
    unique_id: u64,
    /// Metadata partition block of its file entry.
    fe: u32,
    /// Directory: metadata partition block of its identifiers. File: physical block of its data.
    data: u32,
}

enum Kind {
    Dir(Vec<usize>),
    File { path: PathBuf, len: u64 },
}

/// Adds the contents of `dir`, sorted by name, as children of entry `parent`.
fn scan(dir: &Path, parent: usize, entries: &mut Vec<Entry>) -> io::Result<()> {
    let mut items: Vec<_> = fs::read_dir(dir)?.collect::<Result<_, _>>()?;
    items.sort_by_key(|e| e.file_name());
    for item in items {
        let name = item
            .file_name()
            .into_string()
            .map_err(|n| invalid(format!("{n:?}: not a UTF-8 name")))?;
        if name.len() > 254 || !name.is_ascii() {
            return Err(invalid(format!(
                "{name:?}: names must be ASCII, up to 254 characters"
            )));
        }
        // Following symlinks: they are recorded as what they point to.
        let meta = fs::metadata(item.path())?;
        let kind = if meta.is_dir() {
            Kind::Dir(Vec::new())
        } else if meta.is_file() {
            Kind::File {
                path: item.path(),
                len: meta.len(),
            }
        } else {
            return Err(invalid(format!(
                "{}: only files and directories can be written",
                item.path().display()
            )));
        };
        let i = entries.len();
        entries.push(Entry {
            name,
            parent,
            kind,
            unique_id: 15 + i as u64,
            fe: 0,
            data: 0,
        });
        if let Kind::Dir(children) = &mut entries[parent].kind {
            children.push(i);
        }
        if meta.is_dir() {
            scan(&item.path(), i, entries)?;
        }
    }
    Ok(())
}

/// Where everything goes. Blocks are relative to the physical partition, except `fe`
/// and directory `data`, which are relative to the metadata partition.
struct Layout {
    meta_blocks: u32,
    meta_start: u32,
    mirror_start: u32,
    partition_blocks: u32,
    sectors: u32,
}

impl Layout {
    fn new(entries: &mut [Entry]) -> Self {
        let mut meta = 2; // file set descriptor, terminator
        for i in 0..entries.len() {
            entries[i].fe = meta;
            meta += 1;
            if let Kind::Dir(children) = &entries[i].kind {
                entries[i].data = meta;
                meta += blocks(directory_len(entries, children));
            }
        }
        let meta_blocks = meta.div_ceil(META_UNIT) * META_UNIT;
        let meta_start = META_UNIT; // after the metadata file entries, aligned
        let mirror_start = meta_start + meta_blocks;
        let mut next = mirror_start + meta_blocks;
        for e in entries.iter_mut() {
            if let Kind::File { len, .. } = e.kind {
                e.data = next;
                next += blocks(len);
            }
        }
        Layout {
            meta_blocks,
            meta_start,
            mirror_start,
            partition_blocks: next,
            sectors: PARTITION_START + next + 1,
        }
    }
}

fn blocks(bytes: u64) -> u32 {
    bytes.div_ceil(BLOCK as u64) as u32
}

/// A file's data as extents of at most MAX_EXTENT bytes.
fn extents(len: u64, start: u32) -> Vec<(u64, u32, u16)> {
    let mut ads = Vec::new();
    let mut done = 0;
    while done < len {
        let n = (len - done).min(MAX_EXTENT);
        ads.push((n, start + blocks(done), PHYSICAL));
        done += n;
    }
    ads
}

// ----------------------------------------------------------------- directories

fn fid_len(name: &str) -> usize {
    let l_fi = if name.is_empty() { 0 } else { name.len() + 1 };
    (38 + l_fi).div_ceil(4) * 4
}

fn directory_len(entries: &[Entry], children: &[usize]) -> u64 {
    (fid_len("")
        + children
            .iter()
            .map(|&c| fid_len(&entries[c].name))
            .sum::<usize>()) as u64
}

/// The file identifiers of directory `dir`: its parent, then its children.
fn directory(entries: &[Entry], dir: usize, children: &[usize]) -> Vec<u8> {
    let start = entries[dir].data;
    let mut stream = Vec::new();
    let parent = &entries[entries[dir].parent];
    let mut add = |characteristics: u8, name: &str, target: &Entry| {
        let location = start + (stream.len() / BLOCK as usize) as u32;
        stream.extend(fid(characteristics, name, target, location));
    };
    add(0x0a, "", parent); // directory, parent
    for &c in children {
        let child = &entries[c];
        add(
            if matches!(child.kind, Kind::Dir(_)) {
                0x02
            } else {
                0
            },
            &child.name,
            child,
        );
    }
    stream
}

/// File identifier descriptor (ECMA-167 4/14.4).
fn fid(characteristics: u8, name: &str, target: &Entry, location: u32) -> Vec<u8> {
    let mut d = vec![0; fid_len(name)];
    le16(&mut d, 16, 1); // file version
    d[18] = characteristics;
    if !name.is_empty() {
        d[19] = name.len() as u8 + 1;
        d[38] = 8; // CS0, 8 bits per character
        d[39..39 + name.len()].copy_from_slice(name.as_bytes());
    }
    long_ad(&mut d, 20, BLOCK, target.fe, METADATA, target.unique_id);
    tag(&mut d, TAG_FID, location);
    d
}

// ------------------------------------------------------- volume structures

/// PVD, IUVD, PD, LVD, USD and TD of a volume descriptor sequence at `start`.
fn volume_descriptors(start: u32, volume_id: &str, layout: &Layout, time: &Time) -> Vec<Vec<u8>> {
    // Primary volume descriptor (ECMA-167 3/10.1, UDF 2.2.2)
    let mut pvd = vec![0; 512];
    le32(&mut pvd, 16, 0); // volume descriptor sequence number
    dstring(&mut pvd, 24, 32, volume_id);
    le16(&mut pvd, 56, 1); // volume sequence number
    le16(&mut pvd, 58, 1); // maximum volume sequence number
    le16(&mut pvd, 60, 2); // interchange level
    le16(&mut pvd, 62, 3); // maximum interchange level
    le32(&mut pvd, 64, 1); // character set list: CS0
    le32(&mut pvd, 68, 1);
    // volume set identifier: 16 unique hex digits first (UDF 2.2.2.5)
    dstring(
        &mut pvd,
        72,
        128,
        &format!("{:016X}{volume_id}", time.unique),
    );
    charspec(&mut pvd, 200);
    charspec(&mut pvd, 264);
    implementation(&mut pvd, 388);
    timestamp(&mut pvd, 376, time);
    tag(&mut pvd, TAG_PVD, start);

    // Implementation use volume descriptor: logical volume information (UDF 2.2.7)
    let mut iuvd = vec![0; 512];
    le32(&mut iuvd, 16, 1);
    entity(&mut iuvd, 20, "*UDF LV Info", &udf_suffix());
    charspec(&mut iuvd, 52);
    dstring(&mut iuvd, 116, 128, volume_id);
    implementation(&mut iuvd, 352);
    tag(&mut iuvd, TAG_IUVD, start + 1);

    // Partition descriptor (ECMA-167 3/10.5, UDF 2.2.14): read-only, no free space
    let mut pd = vec![0; 512];
    le32(&mut pd, 16, 2);
    le16(&mut pd, 20, 1); // allocated
    le16(&mut pd, 22, 0); // partition number
    entity(&mut pd, 24, "+NSR03", &[]);
    le32(&mut pd, 184, 1); // access type: read-only
    le32(&mut pd, 188, PARTITION_START);
    le32(&mut pd, 192, layout.partition_blocks);
    implementation(&mut pd, 196);
    tag(&mut pd, TAG_PD, start + 2);

    // Logical volume descriptor (ECMA-167 3/10.6, UDF 2.2.4)
    let mut maps = vec![0; 6 + 64];
    maps[0] = 1; // type 1: the physical partition
    maps[1] = 6;
    le16(&mut maps, 2, 1); // volume sequence number
    le16(&mut maps, 4, 0); // partition number
    let m = &mut maps[6..];
    m[0] = 2; // type 2: metadata partition (UDF 2.2.10)
    m[1] = 64;
    entity(m, 4, "*UDF Metadata Partition", &udf_suffix());
    le16(m, 36, 1); // volume sequence number
    le16(m, 38, 0); // partition number
    le32(m, 40, 0); // metadata file location
    le32(m, 44, 1); // metadata mirror file location
    le32(m, 48, 0xffff_ffff); // metadata bitmap file: none on read-only media
    le32(m, 52, META_UNIT); // allocation unit size
    le16(m, 56, META_UNIT as u16); // alignment unit size
    m[58] = 1; // duplicate metadata: the mirror has its own copy

    let mut lvd = vec![0; 440 + maps.len()];
    le32(&mut lvd, 16, 3);
    charspec(&mut lvd, 20);
    dstring(&mut lvd, 84, 128, volume_id);
    le32(&mut lvd, 212, BLOCK);
    entity(&mut lvd, 216, "*OSTA UDF Compliant", &domain_suffix());
    long_ad(&mut lvd, 248, 2 * BLOCK, 0, METADATA, 0); // file set descriptor sequence
    le32(&mut lvd, 264, maps.len() as u32);
    le32(&mut lvd, 268, 2);
    implementation(&mut lvd, 272);
    le32(&mut lvd, 432, 2 * BLOCK); // integrity sequence
    le32(&mut lvd, 436, INTEGRITY);
    lvd[440..].copy_from_slice(&maps);
    tag(&mut lvd, TAG_LVD, start + 3);

    // Unallocated space descriptor: none (ECMA-167 3/10.8)
    let mut usd = vec![0; 24];
    le32(&mut usd, 16, 4);
    tag(&mut usd, TAG_USD, start + 4);

    vec![pvd, iuvd, pd, lvd, usd, terminator(start + 5)]
}

/// Anchor volume descriptor pointer (ECMA-167 3/10.2).
fn anchor(sector: u32) -> Vec<u8> {
    let mut d = vec![0; 512];
    le32(&mut d, 16, VDS_SECTORS * BLOCK);
    le32(&mut d, 20, MAIN_VDS);
    le32(&mut d, 24, VDS_SECTORS * BLOCK);
    le32(&mut d, 28, RESERVE_VDS);
    tag(&mut d, TAG_AVDP, sector);
    d
}

/// Terminating descriptor (ECMA-167 3/10.9).
fn terminator(location: u32) -> Vec<u8> {
    let mut d = vec![0; 512];
    tag(&mut d, TAG_TD, location);
    d
}

/// Logical volume integrity descriptor, closed (ECMA-167 3/10.10, UDF 2.2.6).
fn integrity(entries: &[Entry], layout: &Layout, time: &Time) -> Vec<u8> {
    let mut d = vec![0; 80 + 2 * 8 + 46];
    timestamp(&mut d, 16, time);
    le32(&mut d, 28, 1); // close integrity
    le64(&mut d, 40, 16 + entries.len() as u64); // next unique id
    le32(&mut d, 72, 2); // partitions
    le32(&mut d, 76, 46); // implementation use length
    // free space table (80, 84): 0; size table:
    le32(&mut d, 88, layout.partition_blocks);
    le32(&mut d, 92, layout.meta_blocks);
    implementation(&mut d, 96);
    let dirs = entries
        .iter()
        .filter(|e| matches!(e.kind, Kind::Dir(_)))
        .count() as u32;
    le32(&mut d, 128, entries.len() as u32 - dirs); // files
    le32(&mut d, 132, dirs - 1); // directories, without the root
    le16(&mut d, 136, REVISION); // minimum UDF read revision
    le16(&mut d, 138, REVISION); // minimum UDF write revision
    le16(&mut d, 140, REVISION); // maximum UDF write revision
    tag(&mut d, TAG_LVID, INTEGRITY);
    d
}

/// File set descriptor, at metadata block 0 (ECMA-167 4/14.1, UDF 2.3.2).
fn file_set(volume_id: &str, entries: &[Entry], time: &Time) -> Vec<u8> {
    let mut d = vec![0; 512];
    timestamp(&mut d, 16, time);
    le16(&mut d, 28, 3); // interchange level
    le16(&mut d, 30, 3);
    le32(&mut d, 32, 1); // character set list: CS0
    le32(&mut d, 36, 1);
    charspec(&mut d, 48);
    dstring(&mut d, 112, 128, volume_id);
    charspec(&mut d, 240);
    dstring(&mut d, 304, 32, volume_id);
    long_ad(&mut d, 400, BLOCK, entries[0].fe, METADATA, 0); // root directory
    entity(&mut d, 416, "*OSTA UDF Compliant", &domain_suffix());
    tag(&mut d, TAG_FSD, 0);
    d
}

enum Ads {
    /// (length, block), in the partition of the file entry.
    Short(Vec<(u64, u32)>),
    /// (length, block, partition reference).
    Long(Vec<(u64, u32, u16)>),
}

/// Extended file entry (ECMA-167 4/14.17, UDF 2.3.6), at `location` in its partition.
#[allow(clippy::too_many_arguments)]
fn efe(
    file_type: u8,
    len: u64,
    ads: &Ads,
    unique_id: u64,
    links: u16,
    permissions: u32,
    time: &Time,
    location: u32,
) -> io::Result<Vec<u8>> {
    let (ad_type, ad_bytes, recorded) = match ads {
        Ads::Short(v) => (
            0,
            v.len() * 8,
            v.iter().map(|a| blocks(a.0) as u64).sum::<u64>(),
        ),
        Ads::Long(v) => (1, v.len() * 16, v.iter().map(|a| blocks(a.0) as u64).sum()),
    };
    if 216 + ad_bytes > BLOCK as usize {
        return Err(invalid(format!(
            "file of {len} bytes needs too many extents"
        )));
    }
    let mut d = vec![0; 216 + ad_bytes];
    // ICB tag (ECMA-167 4/14.6)
    le16(&mut d, 20, 4); // strategy type 4
    le16(&mut d, 24, 1); // maximum entries
    d[27] = file_type;
    le16(&mut d, 34, ad_type);
    le32(&mut d, 36, 0xffff_ffff); // uid: unknown
    le32(&mut d, 40, 0xffff_ffff); // gid: unknown
    le32(&mut d, 44, permissions);
    le16(&mut d, 48, links);
    le64(&mut d, 56, len); // information length
    le64(&mut d, 64, len); // object size
    le64(&mut d, 72, recorded);
    for at in [80, 92, 104, 116] {
        timestamp(&mut d, at, time);
    }
    le32(&mut d, 128, 1); // checkpoint
    implementation(&mut d, 168);
    le64(&mut d, 200, unique_id);
    le32(&mut d, 212, ad_bytes as u32);
    match ads {
        Ads::Short(v) => {
            for (i, &(len, block)) in v.iter().enumerate() {
                le32(&mut d, 216 + i * 8, len as u32);
                le32(&mut d, 220 + i * 8, block);
            }
        }
        Ads::Long(v) => {
            for (i, &(len, block, part)) in v.iter().enumerate() {
                long_ad(&mut d, 216 + i * 16, len as u32, block, part, 0);
            }
        }
    }
    tag(&mut d, TAG_EFE, location);
    Ok(d)
}

// ------------------------------------------------------------------ field helpers

fn le16(d: &mut [u8], at: usize, v: u16) {
    d[at..at + 2].copy_from_slice(&v.to_le_bytes());
}

fn le32(d: &mut [u8], at: usize, v: u32) {
    d[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

fn le64(d: &mut [u8], at: usize, v: u64) {
    d[at..at + 8].copy_from_slice(&v.to_le_bytes());
}

/// Fills in the descriptor tag (ECMA-167 3/7.2): version 3, CRC over the rest of `d`,
/// checksum, and the block it is recorded at.
fn tag(d: &mut [u8], id: u16, location: u32) {
    le16(d, 0, id);
    le16(d, 2, 3);
    let crc = crc_itu(&d[16..]);
    le16(d, 8, crc);
    le16(d, 10, (d.len() - 16) as u16);
    le32(d, 12, location);
    d[4] = (0..16)
        .filter(|&i| i != 4)
        .fold(0u8, |sum, i| sum.wrapping_add(d[i]));
}

/// CRC-ITU-T, polynomial 0x1021, initial value 0 (ECMA-167 1/7.2.6).
fn crc_itu(data: &[u8]) -> u16 {
    let mut crc = 0u16;
    for &b in data {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// Character set specification: OSTA CS0 (UDF 2.1.2).
fn charspec(d: &mut [u8], at: usize) {
    d[at + 1..at + 24].copy_from_slice(b"OSTA Compressed Unicode");
}

/// d-string of `len` bytes: 8-bit CS0 characters, recorded length in the last byte.
fn dstring(d: &mut [u8], at: usize, len: usize, s: &str) {
    let s = &s.as_bytes()[..s.len().min(len - 2)];
    d[at] = 8;
    d[at + 1..at + 1 + s.len()].copy_from_slice(s);
    d[at + len - 1] = s.len() as u8 + 1;
}

/// Entity identifier (ECMA-167 1/7.4, UDF 2.1.5).
fn entity(d: &mut [u8], at: usize, id: &str, suffix: &[u8]) {
    d[at + 1..at + 1 + id.len()].copy_from_slice(id.as_bytes());
    d[at + 24..at + 24 + suffix.len()].copy_from_slice(suffix);
}

fn implementation(d: &mut [u8], at: usize) {
    entity(d, at, "*udf-image", &[]);
}

/// Domain identifier suffix: UDF revision, no write protection flags.
fn domain_suffix() -> [u8; 3] {
    let [lo, hi] = REVISION.to_le_bytes();
    [lo, hi, 0]
}

/// UDF identifier suffix: UDF revision, OS class and identifier (undefined).
fn udf_suffix() -> [u8; 4] {
    let [lo, hi] = REVISION.to_le_bytes();
    [lo, hi, 0, 0]
}

/// Long allocation descriptor (ECMA-167 4/14.14.2); its implementation use holds the
/// UDF unique id (UDF 2.3.4.3).
fn long_ad(d: &mut [u8], at: usize, len: u32, block: u32, partition: u16, unique_id: u64) {
    le32(d, at, len);
    le32(d, at + 4, block);
    le16(d, at + 8, partition);
    le32(d, at + 12, unique_id as u32);
}

struct Time {
    year: u16,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
    /// Seconds since the epoch and a hash of the volume id, for the volume set identifier.
    unique: u64,
}

impl Time {
    fn new(time: SystemTime, volume_id: &str) -> Time {
        let secs = time
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
        let days = (secs / 86400) as i64 + 719_468;
        let era = days.div_euclid(146_097);
        let doe = days - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = (doy - (153 * mp + 2) / 5 + 1) as u8;
        let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u8;
        let year = (yoe + era * 400 + (month <= 2) as i64) as u16;
        let rem = secs % 86400;
        Time {
            year,
            month,
            day,
            hour: (rem / 3600) as u8,
            minute: (rem / 60 % 60) as u8,
            second: (rem % 60) as u8,
            unique: secs << 32 | crc_itu(volume_id.as_bytes()) as u64,
        }
    }
}

/// Timestamp (ECMA-167 1/7.3): local time with a 0 offset, i.e. UTC.
fn timestamp(d: &mut [u8], at: usize, t: &Time) {
    le16(d, at, 1 << 12);
    le16(d, at + 2, t.year);
    d[at + 4] = t.month;
    d[at + 5] = t.day;
    d[at + 6] = t.hour;
    d[at + 7] = t.minute;
    d[at + 8] = t.second;
}

fn invalid(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, msg)
}
