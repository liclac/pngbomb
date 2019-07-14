pub mod errors;

use crc::crc32::{self, Hasher32};
use docopt::Docopt;
use error_chain::{bail, quick_main};
use errors::Result;
use flate2::{bufread::ZlibEncoder, Compression};
use pbr::ProgressBar;
use serde::Deserialize;
use std::fs::File;
use std::io;
use std::io::prelude::*;

/// A BufRead implementation which just yields a set number of zeroes.
pub struct ZeroReader {
    pub count: usize,
    pub at: usize,
}

impl ZeroReader {
    pub fn new(count: usize) -> Self {
        Self { count, at: 0 }
    }
}

impl Read for ZeroReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut num = 0;
        for c in buf.iter_mut() {
            if self.at == self.count {
                break;
            }
            *c = 0;
            num += 1;
            self.at += 1;
        }
        Ok(num)
    }
}

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
        compression: png::Compression::Best,
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

    // PNG bitmap data is grouped in "scanlines", eg. data for one horizontal line, prefixed with
    // a 1-byte filter mode flag. We're using no filters (0) and all-black (0) pixels, we just want
    // to generate a whole pile of deflated zeroes, but without allocating it all upfront.
    let ibytes = info.raw_row_length() * height;
    let idata = ZeroReader::new(ibytes);
    let mut zdata = ZlibEncoder::new(
        io::BufReader::with_capacity(64 * 1024, idata),
        Compression::new(4),
    );

    // Write it to an IDAT chunk.
    let mut pb = ProgressBar::new(ibytes as u64);
    pb.set_units(pbr::Units::Bytes);
    pb.message("IDAT: ");
    let mut idat = ChunkWriter::begin(out, png::chunk::IDAT, None)?;
    let mut buf = [0; 2 * 1024 * 1024];
    loop {
        let len = zdata.read(&mut buf[..])?;
        if len == 0 {
            break;
        }
        idat.write_all(&buf[..len])?;
        pb.add(len as u64);
    }
    pb.finish();
    out = idat.finish()?;

    // Write the IEND chunk.
    print!("IEND: ");
    out = write_chunk(out, png::chunk::IEND, &[])?;
    println!("done!");

    Ok(out)
}

const USAGE: &str = "
pngbomb - generate a very big PNG

Usage: pngbomb [options] <outfile>

Options:
  -w PX --width=PX   Output width [default: 10000]
  -h PX --height=PX  Output height [default: 10000]
";

#[derive(Deserialize)]
struct Args {
    arg_outfile: String,
    flag_width: usize,
    flag_height: usize,
}

fn run() -> Result<()> {
    let args: Args = Docopt::new(USAGE)?.deserialize()?;
    render(
        &mut File::create(args.arg_outfile)?,
        args.flag_width,
        args.flag_height,
        png::ColorType::Grayscale,
        png::BitDepth::One,
    )?;
    Ok(())
}
quick_main!(run);
