use std::process::Stdio;

use anyhow::{Result, bail};
use mp4_atom::{Atom, FourCC, Header, Moov, ReadAtom, ReadFrom, Stbl, WriteTo};
use tokio::{io::AsyncWriteExt, process::Command};

// makes timestamps for common framerates integral
const TARGET_TIMESCALE: u32 = 24 * 25 * 30;

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
    moov.mvhd.timescale = TARGET_TIMESCALE;

    let new_stbl_atom = Stbl::read_from(&mut new_stbl)?;
    for track in &mut moov.trak {
        track.mdia.mdhd.timescale = TARGET_TIMESCALE;

        let width = track.tkhd.width.integer();
        let height = track.tkhd.height.integer();

        let mut new_stbl_atom = new_stbl_atom.clone();
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
    }

    let leading_data = original_mp4[..moov_desc.start - 8].into_iter().cloned();
    let trailing_data = original_mp4[moov_desc.end..].into_iter().cloned();

    let mut new_mp4 = leading_data.collect::<Vec<u8>>();
    moov.write_to(&mut new_mp4)?;
    new_mp4.extend(trailing_data);

    return Ok(new_mp4);
}

pub async fn transcode_segment(init_segment: &[u8], video_segment: &[u8]) -> Result<Vec<u8>> {
    let mut gstreamer = Command::new("./gst_transcode");
    gstreamer
        .arg(TARGET_TIMESCALE.to_string())
        .stdout(Stdio::piped())
        .stdin(Stdio::piped())
        .stderr(Stdio::null());

    let mut child = gstreamer.spawn()?;
    let Some(mut stdin) = child.stdin.take() else {
        bail!("Failed to open stdin for gstreamer process");
    };
    stdin.write_all(init_segment).await?;
    stdin.write_all(video_segment).await?;
    drop(stdin);

    let op = child.wait_with_output().await?;
    if !op.status.success() {
        bail!("GStreamer process failed with status: {}", op.status);
    } else {
        let mut transcoded = op.stdout;
        // gstreamer prepends a Moov atom to the output
        let Some(moov_desc) = find_atom(&transcoded, &Moov::KIND) else {
            bail!("No moov atom found in the transcoded output");
        };
        transcoded.drain(..moov_desc.end);
        Ok(transcoded)
    }
}
