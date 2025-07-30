use anyhow::{Result, bail};
use mp4_atom::{Atom, FourCC, Header, Moov, ReadAtom, ReadFrom, Stbl, WriteTo};

#[derive(Debug, Clone)]
pub struct AtomDescription {
    pub header: Header,
    pub start: usize,
    pub end: usize,
}

impl AtomDescription {
    pub fn extract_from<'a>(&self, buffer: &'a [u8]) -> Option<&'a [u8]> {
        if self.start <= buffer.len() && self.end <= buffer.len() {
            Some(&buffer[self.start..self.end])
        } else {
            None
        }
    }

    pub fn extract_from_unchecked<'a>(&self, buffer: &'a [u8]) -> &'a [u8] {
        self.extract_from(buffer).unwrap()
    }
}

pub fn find_atom(data: &[u8], atom: &FourCC) -> Option<AtomDescription> {
    let mut offset = 0;
    while offset < data.len() {
        let Ok(header) = Header::read_from(&mut &data[offset..]) else {
            return None;
        };
        if &header.kind == atom {
            let start = offset + 8;
            let mut end: usize = data.len();
            if let Some(size) = header.size {
                end = start + size as usize;
            }
            return Some(AtomDescription { header, start, end });
        } else if let Some(size) = header.size {
            offset += size as usize + 8; // size doesn't include 8 bytes for the header
        } else {
            // no more atoms to read
            return None;
        }
    }
    None
}

pub fn replace_stbl_atom(original_mp4: &[u8], mut new_stbl: &[u8]) -> Result<Vec<u8>> {
    let Some(moov_desc) = find_atom(original_mp4, &Moov::KIND) else {
        bail!("No moov atom found in the MP4 file");
    };

    let mut moov = Moov::read_atom(
        &moov_desc.header,
        &mut moov_desc.extract_from_unchecked(original_mp4),
    )?;
    let Some(track) = moov.trak.first_mut() else {
        bail!("No track found in the MP4 file");
    };

    let width = track.tkhd.width.integer();
    let height = track.tkhd.height.integer();

    let mut new_stbl_atom = Stbl::read_from(&mut new_stbl)?;
    for codec in &mut new_stbl_atom.stsd.codecs {
        let visual = match codec {
            mp4_atom::Codec::Vp08(vp8) => &mut vp8.visual,
            mp4_atom::Codec::Vp09(vp9) => &mut vp9.visual,
            _ => {
                bail!("New stbl atom not using VP8/VP9 codec");
            }
        };
        visual.width = width;
        visual.height = height;
    }
    track.mdia.minf.stbl = new_stbl_atom;

    let mut new_mp4 = Vec::new();
    moov.write_to(&mut new_mp4)?;

    return Ok(new_mp4);
}
