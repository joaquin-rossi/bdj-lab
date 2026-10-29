//! Playlists of several clips (BDMV/PLAYLIST/NNNNN.mpls), built from the single-clip
//! playlists tsMuxeR writes: their PlayItem (clip, in/out times and stream table) is
//! copied as is, and entry marks are added at the start of the chosen items.
//!
//! Layout as parsed by libbluray (src/libbluray/bdnav/mpls_parse.c).

use anyhow::{Result, bail};

fn u16_at(d: &[u8], pos: usize) -> u16 {
    u16::from_be_bytes([d[pos], d[pos + 1]])
}

/// Offset of the UO mask table in AppInfo: after its length(32), reserved(8),
/// playback type(8) and playback count(16).
const APP_INFO_UO_MASK: usize = 8;

/// Sets the UO mask table of an MPLS file: user operations not allowed while it plays.
pub fn set_uo_mask(mpls: &mut [u8], mask: u64) {
    let at = 40 + APP_INFO_UO_MASK;
    mpls[at..at + 8].copy_from_slice(&mask.to_be_bytes());
}

/// Makes the first PlayItem of an MPLS file hold its last picture on screen when it ends
/// (an infinite still) instead of ending the playlist. PlayItem: length(16), clip(40),
/// codec(32), flags(16), STC id(8), in/out times(64), UO mask(64), random access(8),
/// then the still mode(8): 2 is infinite.
pub fn set_infinite_still(mpls: &mut [u8]) {
    let list_pos = u32_at(mpls, 8) as usize;
    mpls[list_pos + 10 + 31] = 2;
}

fn u32_at(d: &[u8], pos: usize) -> u32 {
    u32::from_be_bytes([d[pos], d[pos + 1], d[pos + 2], d[pos + 3]])
}

/// A single-clip playlist written by tsMuxeR.
pub struct ClipPlaylist {
    app_info: Vec<u8>,
    /// PlayItem, with its length field.
    item: Vec<u8>,
    /// Times of its entry marks (chapters), in 45 kHz ticks of the clip.
    marks: Vec<u32>,
}

impl ClipPlaylist {
    pub fn parse(d: &[u8]) -> Result<Self> {
        if d.len() < 40 || &d[..4] != b"MPLS" {
            bail!("not an MPLS file");
        }
        let list_pos = u32_at(d, 8) as usize;
        // list: length(32), reserved(16), number of items(16), number of sub-paths(16)
        if u16_at(d, list_pos + 6) != 1 || u16_at(d, list_pos + 8) != 0 {
            bail!("expected one PlayItem and no sub-paths");
        }
        let item_start = list_pos + 10;
        let item_len = u16_at(d, item_start) as usize;
        // marks: length(32), number(16), then 14 bytes each: reserved(8), type(8),
        // PlayItem(16), time(32), entry ES PID(16), duration(32)
        let mark_pos = u32_at(d, 12) as usize;
        let marks = (0..u16_at(d, mark_pos + 4) as usize)
            .map(|i| mark_pos + 6 + 14 * i)
            .filter(|&m| d[m + 1] == 1)
            .map(|m| u32_at(d, m + 4))
            .collect();
        Ok(ClipPlaylist {
            app_info: d[40..list_pos].to_vec(),
            item: d[item_start..item_start + 2 + item_len].to_vec(),
            marks,
        })
    }

    /// IN_time of the PlayItem: after its length(16), clip name(5), codec(4), flags(16)
    /// and STC id(8).
    fn in_time(&self) -> u32 {
        u32_at(&self.item, 14)
    }
}

/// A playlist playing `items` in order, with the UO mask table `mask`. Its entry marks
/// (chapters) are those of each clip but its start, which is a mark only for the items
/// in `starts` (indexes).
pub fn playlist(items: &[&ClipPlaylist], starts: &[usize], mask: u64) -> Vec<u8> {
    let mut app_info = items[0].app_info.clone();
    app_info[APP_INFO_UO_MASK..APP_INFO_UO_MASK + 8].copy_from_slice(&mask.to_be_bytes());
    let mut list = vec![0, 0];
    list.extend((items.len() as u16).to_be_bytes());
    list.extend(0u16.to_be_bytes()); // sub-paths
    for item in items {
        list.extend(&item.item);
    }

    let mut marks = Vec::new();
    for (i, item) in items.iter().enumerate() {
        if starts.contains(&i) {
            marks.push((i, item.in_time()));
        }
        marks.extend(
            item.marks
                .iter()
                .filter(|&&t| t != item.in_time())
                .map(|&t| (i, t)),
        );
    }
    let mut mark_section = Vec::new();
    mark_section.extend((marks.len() as u16).to_be_bytes());
    for (i, time) in marks {
        mark_section.push(0); // reserved
        mark_section.push(1); // entry mark (chapter)
        mark_section.extend((i as u16).to_be_bytes());
        mark_section.extend(time.to_be_bytes());
        mark_section.extend(0xffffu16.to_be_bytes()); // entry ES PID: none
        mark_section.extend(0u32.to_be_bytes()); // duration
    }

    let list_pos = 40 + app_info.len();
    let mark_pos = list_pos + 4 + list.len();
    let mut out = b"MPLS0200".to_vec();
    out.extend((list_pos as u32).to_be_bytes());
    out.extend((mark_pos as u32).to_be_bytes());
    out.extend(0u32.to_be_bytes()); // extension data: none
    out.extend([0; 20]);
    out.extend(&app_info);
    out.extend((list.len() as u32).to_be_bytes());
    out.extend(list);
    out.extend((mark_section.len() as u32).to_be_bytes());
    out.extend(mark_section);
    out
}
