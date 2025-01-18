use std::{
    collections::HashMap,
    fs::File,
    io::{BufRead, BufReader, BufWriter, Read, Write},
    path::Path,
};

use countio::Counter;
use log::*;
use lzma::{LzmaReader, LzmaWriter};

use crate::{util, Error, FileInfo};

fn read_u32<T: Read>(reader: &mut T) -> Result<u32, Error> {
    let mut buf = [0; 4];
    reader.read_exact(&mut buf)?;
    let val = u32::from_be_bytes(buf);
    Ok(val)
}

fn write_u32<T: Write>(writer: &mut T, value: u32) -> Result<(), Error> {
    writer.write_all(&value.to_be_bytes())?;
    Ok(())
}

fn read_stringz<T: BufRead>(reader: &mut T) -> Result<String, Error> {
    let mut buf = Vec::new();
    reader.read_until(0, &mut buf)?;
    buf.pop(); // Remove the null terminator
    let string = String::from_utf8(buf)?;
    Ok(string)
}

fn write_stringz<T: Write>(writer: &mut T, string: &str) -> Result<(), Error> {
    writer.write_all(&[string.as_bytes(), &[0]].concat())?;
    Ok(())
}

fn skip_exact<T: Read>(reader: &mut T, count: usize) -> Result<(), Error> {
    let mut buf = vec![0; count];
    reader.read_exact(&mut buf)?;
    Ok(())
}

fn align<T: Into<usize> + From<usize>>(value: T, alignment: T) -> T {
    let value = value.into();
    let alignment = alignment.into();
    let aligned = (value + alignment - 1) & !(alignment - 1);
    aligned.into()
}

#[derive(Debug)]
struct LevelEnds {
    compressed_end: u32,
    uncompressed_end: u32,
}

const EXPECTED_SIGNATURE: &str = "UnityWeb";
const EXPECTED_STREAM_VERSION: u32 = 2;
const EXPECTED_PLAYER_VERSION: &str = "fusion-2.x.x";
const EXPECTED_ENGINE_VERSION_BASE: &str = "2";
const DEFAULT_ENGINE_VERSION: &str = "2.5.4b5";

#[derive(Debug)]
pub struct AssetBundleHeader {
    signature: String,
    stream_version: u32,
    player_version: String,
    engine_version: String,
    min_streamed_bytes: u32,
    header_size: u32,
    num_levels: u32,
    min_levels_for_load: u32,
    level_ends: Vec<LevelEnds>,
    bundle_size: u32,
}
impl AssetBundleHeader {
    fn new(level_ends: Vec<LevelEnds>) -> Self {
        let num_levels = level_ends.len() as u32;
        let mut header = Self {
            signature: EXPECTED_SIGNATURE.to_string(),
            stream_version: EXPECTED_STREAM_VERSION,
            player_version: EXPECTED_PLAYER_VERSION.to_string(),
            engine_version: DEFAULT_ENGINE_VERSION.to_string(),
            num_levels,
            min_levels_for_load: 1,
            level_ends,
            bundle_size: 0,
            min_streamed_bytes: 0,
            header_size: 0,
        };
        header.update_sizes();
        header
    }

    fn read<R: Read + BufRead>(reader: &mut R) -> Result<Self, Error> {
        let signature = read_stringz(reader)?;
        if signature != EXPECTED_SIGNATURE {
            return Err(format!(
                "Invalid signature: {}, should be {}",
                signature, EXPECTED_SIGNATURE
            )
            .into());
        }

        let stream_version = read_u32(reader)?;
        if stream_version != EXPECTED_STREAM_VERSION {
            return Err(format!(
                "Unexpected stream version: {}, expected {}",
                stream_version, EXPECTED_STREAM_VERSION
            )
            .into());
        }

        let player_version = read_stringz(reader)?;
        if player_version != EXPECTED_PLAYER_VERSION {
            return Err(format!(
                "Unexpected player version: {}, expected {}",
                player_version, EXPECTED_PLAYER_VERSION
            )
            .into());
        }

        let engine_version = read_stringz(reader)?;
        if !engine_version.starts_with(EXPECTED_ENGINE_VERSION_BASE) {
            return Err(format!(
                "Unexpected engine version: {}, expected {}",
                engine_version, DEFAULT_ENGINE_VERSION
            )
            .into());
        }

        let min_streamed_bytes = read_u32(reader)?;
        let header_size = read_u32(reader)?;
        let min_levels_for_load = read_u32(reader)?;
        let num_levels = read_u32(reader)?;
        if num_levels < min_levels_for_load {
            return Err(format!(
                "Number of levels ({}) is less than the minimum for load ({})",
                num_levels, min_levels_for_load
            )
            .into());
        }

        let mut level_ends = Vec::with_capacity(num_levels as usize);
        for _ in 0..num_levels {
            let compressed_end = read_u32(reader)?;
            let uncompressed_end = read_u32(reader)?;
            level_ends.push(LevelEnds {
                compressed_end,
                uncompressed_end,
            });
        }

        let bundle_size = read_u32(reader)?;

        Ok(Self {
            signature,
            stream_version,
            player_version,
            engine_version,
            min_streamed_bytes,
            header_size,
            num_levels,
            min_levels_for_load,
            level_ends,
            bundle_size,
        })
    }

    fn write<W: Write>(&self, writer: &mut W) -> Result<(), Error> {
        let mut writer = Counter::new(writer);

        write_stringz(&mut writer, &self.signature)?;
        write_u32(&mut writer, self.stream_version)?;
        write_stringz(&mut writer, &self.player_version)?;
        write_stringz(&mut writer, &self.engine_version)?;
        write_u32(&mut writer, self.min_streamed_bytes)?;
        write_u32(&mut writer, self.header_size)?;
        write_u32(&mut writer, self.min_levels_for_load)?;
        write_u32(&mut writer, self.num_levels)?;
        for level in &self.level_ends {
            write_u32(&mut writer, level.compressed_end)?;
            write_u32(&mut writer, level.uncompressed_end)?;
        }
        write_u32(&mut writer, self.bundle_size)?;

        // padding
        let padding_size = self.header_size as usize - writer.writer_bytes();
        writer.write_all(&vec![0; padding_size])?;

        Ok(())
    }

    fn get_size(&self) -> usize {
        let size = self.signature.len() + 1 // signature (+ null byte)
            + 4 // stream_version
            + self.player_version.len() + 1 // player_version (+ null byte)
            + self.engine_version.len() + 1 // engine_version (+ null byte)
            + 4 // min_streamed_bytes
            + 4 // header_size
            + 4 // min_levels_for_load
            + 4 // num_levels
            + self.level_ends.len() * 8 // level_ends
            + 4; // bundle_size
        align(size, 4)
    }

    fn update_sizes(&mut self) {
        self.header_size = self.get_size() as u32;
        self.bundle_size =
            self.header_size + self.level_ends.last().map_or(0, |l| l.compressed_end);
        self.min_streamed_bytes = self.bundle_size;
    }
}

#[derive(Debug)]
struct LevelFileMetadata {
    name: String,
    offset: u32,
    size: u32,
}

#[derive(Debug)]
struct LevelHeader {
    num_files: u32,
    files: Vec<LevelFileMetadata>,
}
impl LevelHeader {
    fn read<R: Read + BufRead>(reader: &mut R) -> Result<Self, Error> {
        let num_files = read_u32(reader)?;
        let mut files = Vec::with_capacity(num_files as usize);
        for _ in 0..num_files {
            let name = read_stringz(reader)?;
            let offset = read_u32(reader)?;
            let size = read_u32(reader)?;
            files.push(LevelFileMetadata { name, offset, size });
        }
        Ok(Self { num_files, files })
    }

    fn write<W: Write>(&self, writer: &mut W) -> Result<(), Error> {
        write_u32(writer, self.num_files)?;
        for file in &self.files {
            write_stringz(writer, &file.name)?;
            write_u32(writer, file.offset)?;
            write_u32(writer, file.size)?;
        }
        Ok(())
    }
}

struct LevelFile {
    name: String,
    data: Vec<u8>,
}
impl std::fmt::Debug for LevelFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LevelFile")
            .field("name", &self.name)
            .field("data", &format_args!("{} bytes", self.data.len()))
            .finish()
    }
}

#[derive(Debug)]
struct Level {
    files: Vec<LevelFile>,
}
impl Level {
    fn read<R: Read>(reader: &mut R) -> Result<Self, Error> {
        let mut reader = Counter::new(BufReader::new(LzmaReader::new_decompressor(reader)?));
        let header = LevelHeader::read(&mut reader)?;

        let mut files = Vec::with_capacity(header.num_files as usize);
        for file in header.files {
            let offset = reader.reader_bytes();
            skip_exact(&mut reader, file.offset as usize - offset)?;
            let mut data = vec![0; file.size as usize];
            reader.read_exact(&mut data)?;
            files.push(LevelFile {
                name: file.name,
                data,
            });
        }
        Ok(Self { files })
    }

    fn write<W: Write>(&self, writer: &mut W, compression: u32) -> Result<usize, Error> {
        let mut writer = Counter::new(LzmaWriter::new_compressor(writer, compression)?);
        let header = self.gen_header();
        header.write(&mut writer)?;

        for (idx, file) in header.files.iter().enumerate() {
            let padding_size = file.offset as usize - writer.writer_bytes();
            writer.write_all(&vec![0; padding_size])?;
            writer.write_all(&self.files[idx].data)?;
        }

        // pad to 4 bytes
        let level_size = writer.writer_bytes();
        let padding_size = align(level_size, 4) - level_size;
        writer.write_all(&vec![0; padding_size])?;

        let total_written = writer.writer_bytes();
        writer.into_inner().finish()?;
        Ok(total_written)
    }

    fn gen_header(&self) -> LevelHeader {
        let mut files = Vec::with_capacity(self.files.len());

        let header_size = self.get_header_size();
        let mut offset = align(header_size, 4);
        for file in &self.files {
            let size = file.data.len();
            files.push(LevelFileMetadata {
                name: file.name.clone(),
                offset: offset as u32,
                size: size as u32,
            });

            // always align to 4 bytes for the next file
            offset = align(offset + size, 4);
        }

        LevelHeader {
            num_files: self.files.len() as u32,
            files,
        }
    }

    fn get_header_size(&self) -> usize {
        4 // num_files
            + self.files.iter().map(|file| {
                file.name.len() + 1 // name (+ null byte)
                    + 4 // offset
                    + 4 // size
            })
            .sum::<usize>()
    }
}

#[derive(Debug)]
pub struct AssetBundle {
    levels: Vec<Level>,
}
impl AssetBundle {
    fn read<R: Read + BufRead>(reader: &mut R, expected_size: u32) -> Result<Self, Error> {
        let mut reader = Counter::new(reader);

        let header = AssetBundleHeader::read(&mut reader)?;
        if header.bundle_size != expected_size {
            warn!(
                "Bundle size mismatch: {} != {}",
                header.bundle_size, expected_size
            );
        }

        // seek to first level
        let offset = reader.reader_bytes();
        skip_exact(&mut reader, header.header_size as usize - offset)?;

        let mut levels = Vec::with_capacity(header.num_levels as usize);
        for i in 0..header.num_levels {
            let level = Level::read(&mut reader)?;
            levels.push(level);
            if i + 1 < header.num_levels {
                let offset = reader.reader_bytes();
                skip_exact(
                    &mut reader,
                    header.level_ends[i as usize + 1].compressed_end as usize - offset,
                )?;
            }
        }

        Ok(Self { levels })
    }

    fn write<W: Write>(&self, writer: &mut W, compression: u32) -> Result<(), Error> {
        let mut buf = Vec::new();
        let mut buf_writer = Counter::new(&mut buf);
        let mut uncompressed_bytes_written = 0;

        let mut level_ends = Vec::new();
        for level in &self.levels {
            uncompressed_bytes_written += level.write(&mut buf_writer, compression)?;
            let uncompressed_end = uncompressed_bytes_written as u32;
            let compressed_end = buf_writer.writer_bytes() as u32;
            level_ends.push(LevelEnds {
                uncompressed_end,
                compressed_end,
            });
        }

        let header = AssetBundleHeader::new(level_ends);
        header.write(writer)?;
        writer.write_all(&buf)?;
        Ok(())
    }

    pub fn from_file(path: &str) -> Result<Self, String> {
        let file = File::open(path).map_err(|e| format!("Couldn't open file {}: {}", path, e))?;
        let metadata = file.metadata().unwrap();
        let mut reader = BufReader::new(file);
        Self::read(&mut reader, metadata.len() as u32)
            .map_err(|e| format!("Couldn't read bundle: {}", e))
    }

    pub fn to_file(&self, path: &str, compression_level: u32) -> Result<(), String> {
        let file =
            File::create(path).map_err(|e| format!("Couldn't create file {}: {}", path, e))?;
        let mut writer = BufWriter::new(file);
        self.write(&mut writer, compression_level)
            .map_err(|e| format!("Couldn't write bundle: {}", e))?;
        writer
            .flush()
            .map_err(|e| format!("Couldn't finish writing bundle: {}", e))?;
        Ok(())
    }

    pub fn get_uncompressed_info(&self, level: usize) -> Result<HashMap<String, FileInfo>, Error> {
        let mut result = HashMap::new();
        if level >= self.levels.len() {
            return Err(format!("Level {} does not exist", level).into());
        }

        for file in &self.levels[level].files {
            let hash = util::get_buffer_hash(&file.data);
            let info = FileInfo {
                hash,
                size: file.data.len() as u64,
            };
            result.insert(file.name.clone(), info);
        }

        Ok(result)
    }

    pub fn extract_files(&self, output_dir: &str) -> Result<(), String> {
        let make_subdirs = self.levels.len() > 1;
        for (i, level) in self.levels.iter().enumerate() {
            let level_dir = if make_subdirs {
                format!("{}/level{}", output_dir, i)
            } else {
                output_dir.to_string()
            };
            util::create_dir_if_needed(&level_dir)
                .map_err(|e| format!("Couldn't create dir {}: {}", level_dir, e))?;

            let dir_path = Path::new(&level_dir);
            for file in &level.files {
                let file_path = dir_path.join(&file.name);
                std::fs::write(&file_path, &file.data).map_err(|e| {
                    format!("Couldn't write file {}/{}: {}", level_dir, file.name, e)
                })?;
            }
        }
        Ok(())
    }
}
