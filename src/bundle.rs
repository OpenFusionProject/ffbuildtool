#![allow(dead_code)]

use std::{
    collections::HashMap,
    fs::File,
    io::{BufRead, BufReader, Read},
    path::Path,
};

use countio::Counter;
use log::*;
use lzma::LzmaReader;

use crate::{util, Error, FileInfo};

fn read_u32<T: Read>(reader: &mut T) -> Result<u32, Error> {
    let mut buf = [0; 4];
    reader.read_exact(&mut buf)?;
    let val = u32::from_be_bytes(buf);
    Ok(val)
}

fn read_u8<T: Read>(reader: &mut T) -> Result<u8, Error> {
    let mut buf = [0; 1];
    reader.read_exact(&mut buf)?;
    Ok(buf[0])
}

fn read_stringz<T: BufRead>(reader: &mut T) -> Result<String, Error> {
    let mut buf = Vec::new();
    reader.read_until(0, &mut buf)?;
    buf.pop(); // Remove the null terminator
    let string = String::from_utf8(buf)?;
    Ok(string)
}

fn skip_exact<T: Read>(reader: &mut T, count: usize) -> Result<(), Error> {
    let mut buf = vec![0; count];
    reader.read_exact(&mut buf)?;
    Ok(())
}

#[derive(Debug)]
struct LevelEnds {
    compressed_offset: u32,
    uncompressed_offset: u32,
}

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
    fn read<R: Read + BufRead>(reader: &mut R) -> Result<Self, Error> {
        const EXPECTED_SIGNATURE: &str = "UnityWeb";
        const EXPECTED_STREAM_VERSION: u32 = 2;
        const EXPECTED_PLAYER_VERSION: &str = "fusion-2.x.x";
        const EXPECTED_ENGINE_VERSION_BASE: &str = "2";
        const DEFAULT_ENGINE_VERSION: &str = "2.5.4b5";

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
            let compressed_offset = read_u32(reader)?;
            let uncompressed_offset = read_u32(reader)?;
            level_ends.push(LevelEnds {
                compressed_offset,
                uncompressed_offset,
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
}

#[derive(Debug)]
pub struct AssetBundle {
    levels: Vec<Level>,
}
impl AssetBundle {
    pub fn from_file(path: &str) -> Result<Self, Error> {
        let file = File::open(path)?;
        let metadata = file.metadata()?;
        let mut reader = Counter::new(BufReader::new(file));

        let header = AssetBundleHeader::read(&mut reader)?;
        if header.bundle_size != metadata.len() as u32 {
            warn!(
                "Bundle size mismatch: {} != {}",
                header.bundle_size,
                metadata.len()
            );
        }

        #[cfg(debug_assertions)]
        dbg!(&header);

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
                    header.level_ends[i as usize + 1].compressed_offset as usize - offset,
                )?;
            }
        }

        #[cfg(debug_assertions)]
        dbg!(&levels);

        Ok(Self { levels })
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

    pub fn extract_files(&self, output_dir: &str) -> Result<(), Error> {
        let make_subdirs = self.levels.len() > 1;
        for (i, level) in self.levels.iter().enumerate() {
            let level_dir = if make_subdirs {
                format!("{}/level{}", output_dir, i)
            } else {
                output_dir.to_string()
            };
            util::create_dir_if_needed(&level_dir)?;

            let dir_path = Path::new(&level_dir);
            for file in &level.files {
                let file_path = dir_path.join(&file.name);
                std::fs::write(&file_path, &file.data)?;
            }
        }
        Ok(())
    }
}
