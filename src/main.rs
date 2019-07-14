pub mod errors;

use crc::crc32::{self, Hasher32};
use deflate::write::ZlibEncoder;
use error_chain::{bail, quick_main};
use errors::Result;
use pbr::ProgressBar;
use std::fs::File;
use std::io;
use std::io::prelude::*;

pub struct ChunkWriter<W: io::Write + io::Seek> {
    w: W,
    len: Option<usize>,
    crc: crc32::Digest,
    start: u64,
}

impl<W: io::Write + io::Seek> ChunkWriter<W> {
    pub fn begin(mut w: W, typ: [u8; 4], len: Option<usize>) -> Result<Self> {
        // Capture the current position in the file, we need this to rewrite var-length chunks.
        let start = w.seek(io::SeekFrom::Current(0))?;

        // Write the chunk length, if given, or 0 - see finish() for more details.
        // This is not hashed, and does not include itself or the chunk type.
        w.write_all(&(len.unwrap_or(0) as u32).to_be_bytes())?;

        // Write the chunk type, this is hashed, but does not count for the chunk length.
        let mut crc = crc32::Digest::new(crc32::IEEE);
        w.write_all(&typ)?;
        crc.write(&typ);

        Ok(Self { w, len, crc, start })
    }

    pub fn finish(mut self) -> Result<W> {
        // Figure out the block size from the cursor position before/after.
        let cur = self.w.seek(io::SeekFrom::Current(0))?;
        let len = (cur - self.start - 4 - 4) as usize;

        if let Some(hlen) = self.len {
            // This is a known-size block, verify that the length given is correct.
            if len != hlen {
                bail!("wrote {} bytes, but expected {}", len, hlen);
            }
        } else {
            // If not, pop back and rewrite the header.
            self.w.seek(io::SeekFrom::Start(self.start))?;
            self.w.write_all(&(len as u32).to_be_bytes())?;
            self.w.seek(io::SeekFrom::Start(cur))?;
        }

        // Write the CRC32, yield the writer back.
        self.w.write_all(&self.crc.sum32().to_be_bytes())?;
        Ok(self.w)
    }
}

impl<W: io::Write + io::Seek> Write for ChunkWriter<W> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let num = self.w.write(data)?;
        self.crc.write(data);
        Ok(num)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.w.flush()
    }
}

pub fn write_chunk<W: io::Write + io::Seek>(w: W, typ: [u8; 4], data: &[u8]) -> Result<W> {
    let mut cw = ChunkWriter::begin(w, typ, Some(data.len()))?;
    cw.write_all(data)?;
    cw.finish()
}

fn render<W: io::Write + io::Seek>(
    mut out: W,
    width: usize,
    height: usize,
    color_type: png::ColorType,
    bit_depth: png::BitDepth,
) -> Result<W> {
    // Figure out how many bytes of image data to generate.
    // Calculations lifted from the png crate.
    let info = png::Info {
        width: width as u32,
        height: height as u32,
        bit_depth: bit_depth,
        color_type: color_type,
        interlaced: false,
        palette: None,
        trns: None,
        pixel_dims: None,
        frame_control: None,
        animation_control: None,
        compression: png::Compression::Rle,
        filter: png::FilterType::NoFilter,
    };
    //let in_len = info.raw_row_length() - 1;
    //let data_size = in_len * info.height as usize;

    println!(
        "Generating PNG: {}x{}, {}bpp, {:?}",
        width, height, bit_depth as u32, color_type
    );

    print!("Header: ");
    out.write_all(&[137, 80, 78, 71, 13, 10, 26, 10])?;
    println!("done!");

    // Write the IHDR chunk.
    print!("IHDR: ");
    let mut hdr = [0; 13];
    (&mut hdr[..]).write_all(&(width as u32).to_be_bytes())?;
    (&mut hdr[4..]).write_all(&(height as u32).to_be_bytes())?;
    hdr[8] = bit_depth as u8;
    hdr[9] = color_type as u8;
    out = write_chunk(out, png::chunk::IHDR, &hdr)?;
    println!("done!");

    // Generate an IDAT chunk!
    let mut pb = ProgressBar::new(height as u64);
    pb.message("IDAT: ");
    let idat = ChunkWriter::begin(out, png::chunk::IDAT, None)?;
    let zw = ZlibEncoder::new(idat, info.compression.clone());
    let mut w = io::BufWriter::new(zw);
    for row in 0..height {
        w.write_all(&[0x00])?; // Filter method.
        for _ in 0..info.raw_row_length() - 1 {
            w.write_all(&[0x00])?;
        }
        w.flush()?;
        pb.set(row as u64);
    }
    out = w
        .into_inner()
        .map_err(|e| -> errors::Error { format!("{}", e).into() })?
        .finish()?
        .finish()?;
    pb.finish();

    // Write the IEND chunk.
    print!("IEND: ");
    out = write_chunk(out, png::chunk::IEND, &[])?;
    println!("done!");

    Ok(out)
}

fn run() -> Result<()> {
    render(
        &mut File::create("out.png")?,
        10000,
        10000,
        png::ColorType::Grayscale,
        png::BitDepth::One,
    )?;
    Ok(())
}
quick_main!(run);
