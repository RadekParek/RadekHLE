/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Minimal MPEG-4 / QuickTime container demuxer for `MPMoviePlayerController`.
//!
//! Extracts the video track's sample table plus the H.264 SPS/PPS records from
//! the `avcC` sample entry, so an in-process decoder ([super::movie_video]) can
//! render the movie without relying on an external `ffmpeg` binary (which is
//! not available on Android).

#[derive(Debug, Clone)]
pub struct Sample {
    /// Absolute byte offset of the sample inside the movie file.
    pub offset: u64,
    pub size: u32,
    /// Duration in `timescale` units.
    pub duration: u32,
}

#[derive(Debug)]
pub struct DemuxedVideoTrack {
    pub width: u32,
    pub height: u32,
    pub timescale: u32,
    /// Movie/track duration in seconds (mvhd → mdhd → sample-table fallback).
    pub duration_seconds: f64,
    /// SPS/PPS NALs in Annex-B form (`00 00 00 01`-prefixed), from `avcC`.
    pub parameter_sets_annexb: Vec<u8>,
    /// Bytes used for NAL length prefixes inside samples (1, 2 or 4).
    pub nal_length_size: usize,
    /// Video samples in decoding order.
    pub samples: Vec<Sample>,
}

fn read_u32(data: &[u8], off: usize) -> Option<u32> {
    data.get(off..off + 4)
        .map(|b| u32::from_be_bytes(b.try_into().unwrap()))
}

fn read_u64(data: &[u8], off: usize) -> Option<u64> {
    data.get(off..off + 8)
        .map(|b| u64::from_be_bytes(b.try_into().unwrap()))
}

/// Find the (payload_start, payload_end) of a box with the given 4CC inside
/// `data[start..end]`. Returns the first match.
fn find_box(data: &[u8], start: usize, end: usize, kind: &[u8; 4]) -> Option<(usize, usize)> {
    let mut pos = start;
    while pos + 8 <= end {
        let size = read_u32(data, pos)? as usize;
        let box_kind = data.get(pos + 4..pos + 8)?;
        let (header, payload) = if size == 1 {
            // largesize 64-bit
            let large = read_u64(data, pos + 8)? as usize;
            (16, large.saturating_sub(16))
        } else if size == 0 {
            // Box extends to the end of the enclosing scope.
            (8, end - pos - 8)
        } else {
            (8, size.saturating_sub(8))
        };
        if box_kind == kind {
            let payload_start = pos + header;
            let payload_end = payload_start + payload;
            if payload_end <= end {
                return Some((payload_start, payload_end));
            }
            return None;
        }
        if size == 0 {
            break;
        }
        pos += header + payload;
    }
    None
}

/// Iterate over the child boxes of `data[start..end]`.
fn for_each_box<F: FnMut(&[u8; 4], usize, usize) -> Option<bool>>(
    data: &[u8],
    start: usize,
    end: usize,
    f: &mut F,
) {
    let mut pos = start;
    while pos + 8 <= end {
        let Some(size) = read_u32(data, pos) else { return };
        let Some(kind) = data.get(pos + 4..pos + 8) else { return };
        let mut kind_arr = [0u8; 4];
        kind_arr.copy_from_slice(kind);
        let (header, payload) = if size == 1 {
            let Some(large) = read_u64(data, pos + 8) else { return };
            (16, (large as usize).saturating_sub(16))
        } else if size == 0 {
            (8, end - pos - 8)
        } else {
            (8, (size as usize).saturating_sub(8))
        };
        let payload_start = pos + header;
        let payload_end = payload_start + payload;
        if payload_end > end {
            return;
        }
        match f(&kind_arr, payload_start, payload_end) {
            Some(false) => return,
            _ => {}
        }
        if size == 0 {
            return;
        }
        pos = payload_end;
    }
}

/// Parse the `avcC` (AVCDecoderConfigurationRecord) payload.
fn parse_avcc(data: &[u8]) -> Option<(Vec<u8>, usize)> {
    if data.len() < 7 {
        return None;
    }
    let nal_length_size = 1 + (data[4] & 0x03) as usize;
    let num_sps = (data[5] & 0x1f) as usize;
    let mut pos = 6;
    let mut annexb = Vec::new();
    for _ in 0..num_sps {
        if pos + 2 > data.len() {
            return Some((annexb, nal_length_size));
        }
        let len = u16::from_be_bytes([data[pos], data[pos + 1]]) as usize;
        pos += 2;
        let end = (pos + len).min(data.len());
        annexb.extend_from_slice(&[0, 0, 0, 1]);
        annexb.extend_from_slice(&data[pos..end]);
        pos = end;
    }
    if pos >= data.len() {
        return Some((annexb, nal_length_size));
    }
    let num_pps = data[pos] as usize;
    pos += 1;
    for _ in 0..num_pps {
        if pos + 2 > data.len() {
            break;
        }
        let len = u16::from_be_bytes([data[pos], data[pos + 1]]) as usize;
        pos += 2;
        let end = (pos + len).min(data.len());
        annexb.extend_from_slice(&[0, 0, 0, 1]);
        annexb.extend_from_slice(&data[pos..end]);
        pos = end;
    }
    Some((annexb, nal_length_size))
}

/// Parse an `stsd` payload and return (width, height, avcc_annexb, nal_len_size)
/// for the first `avc1` (or `avc3`) video sample entry.
fn parse_stsd(data: &[u8]) -> Option<(u32, u32, Vec<u8>, usize)> {
    if data.len() < 8 {
        return None;
    }
    let entry_count = read_u32(data, 4)?;
    let mut pos = 8;
    for _ in 0..entry_count.min(16) {
        let entry_size = read_u32(data, pos)? as usize;
        if entry_size < 8 || pos + entry_size > data.len() {
            return None;
        }
        let format = &data[pos + 4..pos + 8];
        if format == b"avc1" || format == b"avc3" {
            // Common sample-entry fields: 6 reserved + 2 data_ref_index.
            // Visual sample entry: 16 bytes predefined/reserved, then
            // width, height (u16 each).
            if entry_size < 8 + 16 + 4 {
                return None;
            }
            // Offsets are relative to the entry start: 8-byte box header,
            // 8-byte common part (6 reserved + 2 data_ref_index), then the
            // 70-byte visual part (16 predefined/reserved, then width,
            // height, hres, vres, reserved, frame count, compressor name,
            // depth, pre_defined).
            let width = read_u32(data, pos + 8 + 8 + 16)? >> 16;
            let height = read_u32(data, pos + 8 + 8 + 18)? >> 16;
            // Child boxes (e.g. avcC) begin after the 8-byte box header +
            // 8-byte common part + 70-byte visual part.
            let children_start = pos + 8 + 8 + 70.min(entry_size - 16);
            let children_end = pos + entry_size;
            if let Some((s, e)) = find_box(data, children_start, children_end, b"avcC") {
                if let Some((annexb, nal_len)) = parse_avcc(&data[s..e]) {
                    return Some((width, height, annexb, nal_len));
                }
            }
        }
        pos += entry_size;
    }
    None
}

/// Parse `stts` payload into a flat list of per-sample durations.
fn parse_stts(data: &[u8]) -> Vec<u32> {
    let mut durations = Vec::new();
    if data.len() < 8 {
        return durations;
    }
    let entry_count = read_u32(data, 4).unwrap_or(0);
    let mut pos = 8;
    for _ in 0..entry_count {
        let (Some(count), Some(delta)) = (read_u32(data, pos), read_u32(data, pos + 4)) else {
            break;
        };
        for _ in 0..count {
            durations.push(delta);
        }
        pos += 8;
    }
    durations
}

/// Expand the sample table: chunk offsets (stco/co64) + samples-per-chunk
/// (stsc) + sample sizes (stsz) + per-sample durations (stts).
fn build_samples(
    chunk_offsets: &[u64],
    stsc: &[(u32, u32)],
    stsz_item_size: u32,
    stsz_sizes: &[u32],
    stts_durations: &[u32],
) -> Vec<Sample> {
    let mut samples = Vec::new();
    let mut sample_index: usize = 0;
    for (chunk_idx, &chunk_offset) in chunk_offsets.iter().enumerate() {
        let chunk_no = (chunk_idx + 1) as u32;
        let samples_per_chunk = stsc
            .iter()
            .rev()
            .find(|&&(first_chunk, _)| first_chunk <= chunk_no)
            .map(|&(_, spc)| spc)
            .unwrap_or(0);
        if samples_per_chunk == 0 {
            continue;
        }
        let mut offset = chunk_offset;
        for _ in 0..samples_per_chunk {
            let size = if stsz_item_size != 0 {
                stsz_item_size
            } else {
                match stsz_sizes.get(sample_index) {
                    Some(&s) => s,
                    None => break,
                }
            };
            if size == 0 {
                break;
            }
            // Samples within a chunk are contiguous.
            let duration = stts_durations.get(sample_index).copied().unwrap_or(0);
            samples.push(Sample {
                offset,
                size,
                duration,
            });
            offset += size as u64;
            sample_index += 1;
        }
    }
    samples
}

/// Demux the first H.264 video track from an MP4/MOV file.
pub fn demux_video_track(bytes: &[u8]) -> Option<DemuxedVideoTrack> {
    let end = bytes.len();
    let (moov_start, moov_end) = find_box(bytes, 0, end, b"moov")?;

    // mvhd: movie timescale + duration.
    let movie_duration = find_box(bytes, moov_start, moov_end, b"mvhd")
        .and_then(|(s, e)| {
            let payload = &bytes[s..e];
            if payload.is_empty() {
                return None;
            }
            let version = payload[0];
            if version == 1 && payload.len() >= 28 {
                let timescale = read_u32(payload, 20)?;
                let duration = read_u64(payload, 24)?;
                Some((timescale, duration))
            } else if version == 0 && payload.len() >= 20 {
                let timescale = read_u32(payload, 12)?;
                let duration = read_u32(payload, 16)?;
                Some((timescale, duration as u64))
            } else {
                None
            }
        });

    // Find the video trak.
    let mut result = None;
    for_each_box(bytes, moov_start, moov_end, &mut |kind, s, e| {
        if kind != b"trak" || result.is_some() {
            return None;
        }
        // hdlr must say 'vide'.
        let Some((mdia_start, mdia_end)) = find_box(bytes, s, e, b"mdia") else {
            return Some(true);
        };
        let is_video = find_box(bytes, mdia_start, mdia_end, b"hdlr").is_some_and(|(hs, he)| {
            // handler_type at offset 8 of the FullBox payload.
            bytes.get(hs + 8..hs + 12) == Some(b"vide")
        });
        if !is_video {
            return Some(true);
        }

        // Track timescale/duration from mdhd (fallback when mvhd is absent).
        let track_duration = find_box(bytes, mdia_start, mdia_end, b"mdhd").and_then(|(ms, me)| {
            let payload = &bytes[ms..me];
            if payload.is_empty() {
                return None;
            }
            let version = payload[0];
            if version == 1 && payload.len() >= 28 {
                Some((read_u32(payload, 20)?, read_u64(payload, 24)?))
            } else if version == 0 && payload.len() >= 20 {
                Some((read_u32(payload, 12)?, read_u32(payload, 16)? as u64))
            } else {
                None
            }
        });

        let Some((stbl_start, stbl_end)) =
            find_box(bytes, mdia_start, mdia_end, b"minf").and_then(|(ms, me)| {
                find_box(bytes, ms, me, b"stbl")
            })
        else {
            return Some(true);
        };

        // stsd: codec, dimensions, avcC.
        let Some((stsd_s, stsd_e)) = find_box(bytes, stbl_start, stbl_end, b"stsd") else {
            return Some(true);
        };
        let Some((width, height, annexb, nal_len)) = parse_stsd(&bytes[stsd_s..stsd_e]) else {
            return Some(true);
        };

        // stsz.
        let (item_size, sizes) = find_box(bytes, stbl_start, stbl_end, b"stsz")
            .map(|(s, e)| {
                let payload = &bytes[s..e];
                let item_size = read_u32(payload, 4).unwrap_or(0);
                let count = read_u32(payload, 8).unwrap_or(0) as usize;
                let mut sizes = Vec::with_capacity(count);
                for i in 0..count {
                    if let Some(v) = read_u32(payload, 12 + i * 4) {
                        sizes.push(v);
                    }
                }
                (item_size, sizes)
            })
            .unwrap_or((0, Vec::new()));

        // stsc.
        let mut stsc = Vec::new();
        if let Some((s, e)) = find_box(bytes, stbl_start, stbl_end, b"stsc") {
            let payload = &bytes[s..e];
            let count = read_u32(payload, 4).unwrap_or(0) as usize;
            for i in 0..count {
                let first = read_u32(payload, 8 + i * 12);
                let spc = read_u32(payload, 12 + i * 12);
                if let (Some(f), Some(n)) = (first, spc) {
                    stsc.push((f, n));
                }
            }
        }

        // stco or co64.
        let chunk_offsets: Vec<u64> = if let Some((s, e)) = find_box(bytes, stbl_start, stbl_end, b"stco") {
            let payload = &bytes[s..e];
            let count = read_u32(payload, 4).unwrap_or(0) as usize;
            (0..count).filter_map(|i| read_u32(payload, 8 + i * 4).map(|v| v as u64)).collect()
        } else if let Some((s, e)) = find_box(bytes, stbl_start, stbl_end, b"co64") {
            let payload = &bytes[s..e];
            let count = read_u32(payload, 4).unwrap_or(0) as usize;
            (0..count).filter_map(|i| read_u64(payload, 8 + i * 8)).collect()
        } else {
            Vec::new()
        };

        // stts.
        let stts_durations = find_box(bytes, stbl_start, stbl_end, b"stts")
            .map(|(s, e)| parse_stts(&bytes[s..e]))
            .unwrap_or_default();

        let timescale = track_duration
            .as_ref()
            .map(|(ts, _)| *ts)
            .or(movie_duration.as_ref().map(|(ts, _)| *ts))
            .unwrap_or(600);
        let duration_u = movie_duration
            .as_ref()
            .map(|(_, d)| *d)
            .or(track_duration.as_ref().map(|(_, d)| *d));
        let duration_seconds = match duration_u {
            Some(d) => d as f64 / timescale as f64,
            None => {
                let total: u32 = stts_durations.iter().sum();
                total as f64 / timescale as f64
            }
        };

        let samples = build_samples(&chunk_offsets, &stsc, item_size, &sizes, &stts_durations);
        result = Some(DemuxedVideoTrack {
            width,
            height,
            timescale,
            duration_seconds,
            parameter_sets_annexb: annexb,
            nal_length_size: nal_len,
            samples,
        });
        Some(false)
    });

    let track = result?;
    if track.width == 0 || track.height == 0 || track.samples.is_empty() {
        return None;
    }
    // Validate that sample offsets are within the file (mdat may follow moov).
    if track
        .samples
        .last()
        .is_some_and(|s| s.offset as usize + s.size as usize > bytes.len())
    {
        return None;
    }
    Some(track)
}

/// Convert one AVCC sample (4-byte/2-byte/1-byte length-prefixed NAL units)
/// into Annex-B form, appended to `out`.
pub fn avcc_sample_to_annexb(sample: &[u8], nal_length_size: usize, out: &mut Vec<u8>) {
    let mut pos = 0usize;
    while pos + nal_length_size <= sample.len() {
        let mut len = 0usize;
        for i in 0..nal_length_size {
            len = (len << 8) | sample[pos + i] as usize;
        }
        pos += nal_length_size;
        if len == 0 || pos + len > sample.len() {
            break;
        }
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(&sample[pos..pos + len]);
        pos += len;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn box_bytes(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(payload);
        out
    }

    fn full_box(kind: &[u8; 4], version: u8, flags: u32, payload: &[u8]) -> Vec<u8> {
        let mut p = vec![version];
        p.extend_from_slice(&flags.to_be_bytes()[1..4]);
        p.extend_from_slice(payload);
        box_bytes(kind, &p)
    }

    #[test]
    fn demuxes_synthetic_mp4() {
        // avcC: version, profile, compat, level, len_size_minus_one=3 (4-byte),
        // 1 SPS (len 4, bytes [0x67,1,2,3]), 1 PPS (len 4, [0x68,4,5,6]).
        let avcc = box_bytes(b"avcC", &[
            1, 0x42, 0x00, 0x1e, 0xff, 0xe1, 0, 4, 0x67, 1, 2, 3, 1, 0, 4, 0x68, 4, 5, 6,
        ]);
        // Build the visual sample entry BODY first, then wrap it in the box
        // so the size header covers the whole entry.
        let mut avc1_body = Vec::new();
        avc1_body.extend_from_slice(&[0u8; 6]); // reserved
        avc1_body.extend_from_slice(&1u16.to_be_bytes()); // data_ref_index
        avc1_body.extend_from_slice(&[0u8; 16]); // predefined/reserved
        avc1_body.extend_from_slice(&320u16.to_be_bytes()); // width
        avc1_body.extend_from_slice(&240u16.to_be_bytes()); // height
        avc1_body.extend_from_slice(&[0u8; 12]); // hres/vres/reserved
        avc1_body.extend_from_slice(&1u16.to_be_bytes()); // frame count
        avc1_body.extend_from_slice(&[0u8; 32]); // compressor name
        avc1_body.extend_from_slice(&[0, 0x18]); // depth
        avc1_body.extend_from_slice(&[0xff, 0xff]); // pre_defined
        avc1_body.extend_from_slice(&avcc);
        let avc1 = box_bytes(b"avc1", &avc1_body);
        let mut stsd_payload = 1u32.to_be_bytes().to_vec();
        stsd_payload.extend_from_slice(&avc1);
        let stsd = full_box(b"stsd", 0, 0, &stsd_payload);

        // 4 samples: sizes 10, 20, 30, 40; durations 33 each.
        let mut stsz_payload: Vec<u8> = Vec::new();
        stsz_payload.extend_from_slice(&0u32.to_be_bytes());
        stsz_payload.extend_from_slice(&4u32.to_be_bytes());
        for s in [10u32, 20, 30, 40] {
            stsz_payload.extend_from_slice(&s.to_be_bytes());
        }
        let stsz = full_box(b"stsz", 0, 0, &stsz_payload);

        let mut stts_payload = 1u32.to_be_bytes().to_vec();
        stts_payload.extend_from_slice(&4u32.to_be_bytes());
        stts_payload.extend_from_slice(&33u32.to_be_bytes());
        let stts = full_box(b"stts", 0, 0, &stts_payload);

        // 2 chunks: chunk 1 has 3 samples, chunk 2 has the rest.
        let mut stsc_payload = 2u32.to_be_bytes().to_vec();
        stsc_payload.extend_from_slice(&1u32.to_be_bytes());
        stsc_payload.extend_from_slice(&3u32.to_be_bytes());
        stsc_payload.extend_from_slice(&1u32.to_be_bytes());
        stsc_payload.extend_from_slice(&4u32.to_be_bytes());
        stsc_payload.extend_from_slice(&2u32.to_be_bytes());
        stsc_payload.extend_from_slice(&1u32.to_be_bytes());
        let stsc = full_box(b"stsc", 0, 0, &stsc_payload);

        // Chunk offsets: 1000 and 1060.
        let mut stco_payload = 2u32.to_be_bytes().to_vec();
        stco_payload.extend_from_slice(&1000u32.to_be_bytes());
        stco_payload.extend_from_slice(&1060u32.to_be_bytes());
        let stco = full_box(b"stco", 0, 0, &stco_payload);

        let mut stbl = Vec::new();
        stbl.extend_from_slice(&stsd);
        stbl.extend_from_slice(&stts);
        stbl.extend_from_slice(&stsc);
        stbl.extend_from_slice(&stco);
        stbl.extend_from_slice(&stsz);
        let stbl = box_bytes(b"stbl", &stbl);

        let hdlr = full_box(b"hdlr", 0, 0, &{
            let mut p = vec![0u8; 4]; // pre_defined
            p.extend_from_slice(b"vide");
            p
        });
        // mdhd v0: creation, modification, timescale=600, duration=2400.
        let mut mdhd_payload = vec![0u8; 8];
        mdhd_payload.extend_from_slice(&600u32.to_be_bytes());
        mdhd_payload.extend_from_slice(&2400u32.to_be_bytes());
        mdhd_payload.extend_from_slice(&[0, 0]); // language
        let mdhd = full_box(b"mdhd", 0, 0, &mdhd_payload);

        let mut mdia_children = mdhd;
        mdia_children.extend_from_slice(&hdlr);
        mdia_children.extend_from_slice(&box_bytes(b"minf", &stbl));
        let mdia = box_bytes(b"mdia", &mdia_children);
        let mut trak = box_bytes(b"trak", &mdia);

        // mvhd v0: timescale 600, duration 2400 (4 s).
        // v0 payload after version+flags: creation(4) + modification(4) +
        // timescale(4) + duration(4) + rate(4) + volume(2) + reserved(10) +
        // matrix(36) + pre_defined(24) + next_track_ID(4) = 100 bytes.
        let mut mvhd_payload = vec![0u8; 8];
        mvhd_payload.extend_from_slice(&600u32.to_be_bytes());
        mvhd_payload.extend_from_slice(&2400u32.to_be_bytes());
        mvhd_payload.extend_from_slice(&[0u8; 80]);
        let mvhd = full_box(b"mvhd", 0, 0, &mvhd_payload);

        let mut moov_children = mvhd;
        moov_children.extend_from_slice(&trak);
        let moov = box_bytes(b"moov", &moov_children);

        // Padding to push mdat samples at offsets 1000/1060.
        let mut file = moov;
        file.resize(1000, 0);
        file.extend_from_slice(&[0xAAu8; 60]); // chunk 1 data
        file.extend_from_slice(&[0xBBu8; 100]); // chunk 2 data

        let track = demux_video_track(&file).expect("should demux");
        assert_eq!(track.width, 320);
        assert_eq!(track.height, 240);
        assert_eq!(track.timescale, 600);
        assert!((track.duration_seconds - 4.0).abs() < 1e-6);
        assert_eq!(track.nal_length_size, 4);
        // Annex-B parameter sets: SPS + PPS with start codes.
        assert_eq!(
            &track.parameter_sets_annexb[..8],
            &[0, 0, 0, 1, 0x67, 1, 2, 3]
        );
        assert_eq!(track.samples.len(), 4);
        assert_eq!(track.samples[0].offset, 1000);
        assert_eq!(track.samples[0].size, 10);
        assert_eq!(track.samples[3].offset, 1060); // chunk 2 sample 1 (1000 + 60 bytes of chunk 1)
        assert_eq!(track.samples[3].size, 40);
        assert_eq!(track.samples[2].duration, 33);

        // AVCC → Annex-B conversion.
        let sample: Vec<u8> = [4u32.to_be_bytes().as_slice(), &[0x65, 1, 2, 3][..]].concat();
        let mut out = Vec::new();
        avcc_sample_to_annexb(&sample, 4, &mut out);
        assert_eq!(out, vec![0, 0, 0, 1, 0x65, 1, 2, 3]);
    }
}
