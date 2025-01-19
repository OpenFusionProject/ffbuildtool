use std::{
    collections::HashMap,
    fs::File,
    io::{BufRead, BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
};

use countio::Counter;
use liblzma::{
    read::XzDecoder,
    stream::{LzmaOptions, Stream},
    write::XzEncoder,
};
use log::*;

use crate::{util, Error, FileInfo};

// level index, file index, total files, file name
pub type CompressionCallback = fn(usize, usize, usize, String);

fn get_lzma_encoder<W: Write>(writer: &mut W, level: u32) -> Result<XzEncoder<&mut W>, Error> {
    let mut options = LzmaOptions::new_preset(level)?;
    options
        .literal_context_bits(3)
        .literal_position_bits(0)
        .position_bits(2)
        .dict_size(1 << 23);

    let stream = Stream::new_lzma_encoder(&options)?;
    Ok(XzEncoder::new_stream(writer, stream))
}

fn get_lzma_decoder<R: Read>(reader: &mut R) -> Result<XzDecoder<&mut R>, Error> {
    let stream = Stream::new_lzma_decoder(u64::MAX)?;
    Ok(XzDecoder::new_stream(reader, stream))
}

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
impl std::fmt::Display for AssetBundleHeader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Signature: {}", self.signature)?;
        writeln!(f, "Stream version: {}", self.stream_version)?;
        writeln!(f, "Player version: {}", self.player_version)?;
        writeln!(f, "Engine version: {}", self.engine_version)?;
        writeln!(f, "Min streamed bytes: {}", self.min_streamed_bytes)?;
        writeln!(f, "Header size: {}", self.header_size)?;
        writeln!(f, "Number of levels: {}", self.num_levels)?;
        writeln!(f, "Min levels for load: {}", self.min_levels_for_load)?;
        writeln!(f, "Level ends:")?;
        for (i, level) in self.level_ends.iter().enumerate() {
            writeln!(
                f,
                "  Level {}: compressed @ {}, uncompressed @ {}",
                i, level.compressed_end, level.uncompressed_end
            )?;
        }
        write!(
            f,
            "Bundle size: {} ({} bytes)",
            util::bytes_to_human_readable(self.bundle_size),
            self.bundle_size
        )
    }
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
    hash: Option<String>,
}
impl std::fmt::Debug for LevelFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LevelFile")
            .field("name", &self.name)
            .field("data", &format_args!("{} bytes", self.data.len()))
            .field("hash", &self.hash)
            .finish()
    }
}
impl std::fmt::Display for LevelFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.hash.as_ref() {
            Some(hash) => write!(
                f,
                "{} - {} ({} bytes) - {}",
                self.name,
                util::bytes_to_human_readable(self.data.len() as u32),
                self.data.len(),
                hash
            ),
            None => write!(
                f,
                "{} - {} ({} bytes)",
                self.name,
                util::bytes_to_human_readable(self.data.len() as u32),
                self.data.len()
            ),
        }
    }
}
impl LevelFile {
    fn new(name: String, data: Vec<u8>) -> Self {
        Self {
            name,
            data,
            hash: None,
        }
    }
}

#[derive(Debug)]
struct Level {
    files: Vec<LevelFile>,
}
impl Level {
    fn read<R: Read + BufRead>(reader: &mut R) -> Result<Self, Error> {
        let mut reader = Counter::new(BufReader::new(get_lzma_decoder(reader)?));
        let header = LevelHeader::read(&mut reader)?;

        let mut files = Vec::with_capacity(header.num_files as usize);
        for file in header.files {
            let offset = reader.reader_bytes();
            skip_exact(&mut reader, file.offset as usize - offset)?;
            let mut data = vec![0; file.size as usize];
            reader.read_exact(&mut data)?;
            files.push(LevelFile::new(file.name, data));
        }
        Ok(Self { files })
    }

    fn write<W: Write>(
        &self,
        writer: &mut W,
        compression: u32,
        level_idx: usize,
        callback: Option<CompressionCallback>,
    ) -> Result<usize, Error> {
        let mut writer = Counter::new(get_lzma_encoder(writer, compression)?);
        let header = self.gen_header();
        header.write(&mut writer)?;

        let num_files = header.files.len();
        for (idx, file) in header.files.iter().enumerate() {
            let padding_size = file.offset as usize - writer.writer_bytes();
            writer.write_all(&vec![0; padding_size])?;

            if let Some(callback) = callback {
                callback(level_idx, idx, num_files, file.name.clone());
            }
            writer.write_all(&self.files[idx].data)?;
        }

        if let Some(callback) = callback {
            callback(level_idx, num_files, num_files, "Done".to_string());
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
impl std::fmt::Display for AssetBundle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (i, level) in self.levels.iter().enumerate() {
            writeln!(f, "Level {}", i)?;
            for file in &level.files {
                writeln!(f, "  {}", file)?;
            }
            write!(f, "End")?;
        }
        Ok(())
    }
}
impl AssetBundle {
    fn read<R: Read + BufRead>(
        reader: &mut R,
        expected_size: u32,
    ) -> Result<(AssetBundleHeader, Self), Error> {
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

        Ok((header, Self { levels }))
    }

    fn write<W: Write>(
        &self,
        writer: &mut W,
        compression: u32,
        callback: Option<CompressionCallback>,
    ) -> Result<(), Error> {
        let mut buf = Vec::new();
        let mut buf_writer = Counter::new(&mut buf);
        let mut uncompressed_bytes_written = 0;

        let mut level_sizes_uncompressed = Vec::with_capacity(self.levels.len());
        let mut level_ends = Vec::with_capacity(self.levels.len());
        for (idx, level) in self.levels.iter().enumerate() {
            let level_size_uncompressed =
                level.write(&mut buf_writer, compression, idx, callback)? as u64;
            uncompressed_bytes_written += level_size_uncompressed;
            level_sizes_uncompressed.push(level_size_uncompressed);

            let uncompressed_end = uncompressed_bytes_written as u32;
            let compressed_end = buf_writer.writer_bytes() as u32;
            level_ends.push(LevelEnds {
                uncompressed_end,
                compressed_end,
            });
        }

        // The LZMA_alone encoder does not write the correct buffer sizes
        // to the headers (it writes all 0xFFs), so sub them in.
        for i in 0..self.levels.len() {
            let level_start = if i == 0 {
                0
            } else {
                level_ends[i - 1].compressed_end
            };

            let level_size_uncompressed = level_sizes_uncompressed[i];
            let level_size_uncompressed_start = (level_start
                + 1 // properties byte
                + 4) // dict size
                as usize;

            let slice = &mut buf[level_size_uncompressed_start..level_size_uncompressed_start + 8];
            assert!(slice == [0xFF; 8]);
            slice.copy_from_slice(&level_size_uncompressed.to_le_bytes());
        }

        let header = AssetBundleHeader::new(level_ends);
        header.write(writer)?;
        writer.write_all(&buf)?;
        Ok(())
    }

    fn get_level_files_from_dir(dir_path: &Path) -> Result<Vec<LevelFile>, Error> {
        let mut files = Vec::new();
        for entry in std::fs::read_dir(dir_path)? {
            let Ok(entry) = entry else {
                continue;
            };
            let path = entry.path();
            if path.is_file() {
                let Some(name) = path.file_name().unwrap().to_str() else {
                    continue;
                };
                let Ok(data) = std::fs::read(&path) else {
                    continue;
                };
                files.push(LevelFile::new(name.to_string(), data));
            }
        }
        Ok(files)
    }

    pub fn from_file(path: &str) -> Result<(AssetBundleHeader, Self), String> {
        let file = File::open(path).map_err(|e| format!("Couldn't open file {}: {}", path, e))?;
        let metadata = file.metadata().unwrap();
        let mut reader = BufReader::new(file);
        Self::read(&mut reader, metadata.len() as u32)
            .map_err(|e| format!("Couldn't read bundle: {}", e))
    }

    pub fn from_directory(path: &str) -> Result<Self, String> {
        // each subdirectory with the name `levelX` contains the files for that level.
        // they must be in order-- starting from level0-- for their files to be included.
        // all loose files get put at the end of level0.
        let root_path = PathBuf::from(path);
        if !root_path.is_dir() {
            return Err(format!("Invalid root directory: {}", path));
        }

        let mut levels = Vec::new();
        for i in 0.. {
            let level_dir = root_path.join(format!("level{}", i));
            if !level_dir.as_path().is_dir() {
                break;
            }

            let Ok(files) = Self::get_level_files_from_dir(&level_dir) else {
                return Err(format!(
                    "Couldn't read files in dir: {}",
                    level_dir.display()
                ));
            };

            levels.push(Level { files });
        }

        let Ok(loose_files) = Self::get_level_files_from_dir(&root_path) else {
            return Err(format!(
                "Couldn't read files in dir: {}",
                root_path.display()
            ));
        };
        if levels.is_empty() {
            levels.push(Level { files: loose_files });
        } else {
            levels[0].files.extend(loose_files);
        }

        Ok(Self { levels })
    }

    pub fn to_file(
        &self,
        path: &str,
        compression_level: u32,
        callback: Option<CompressionCallback>,
    ) -> Result<(), String> {
        let file =
            File::create(path).map_err(|e| format!("Couldn't create file {}: {}", path, e))?;
        let mut writer = BufWriter::new(file);
        self.write(&mut writer, compression_level, callback)
            .map_err(|e| format!("Couldn't write bundle: {}", e))?;
        writer
            .flush()
            .map_err(|e| format!("Couldn't finish writing bundle: {}", e))?;
        Ok(())
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

    pub fn recalculate_all_hashes(&mut self) {
        for level in &mut self.levels {
            for file in &mut level.files {
                file.hash = Some(util::get_buffer_hash(&file.data));
            }
        }
    }

    pub fn get_uncompressed_info(&self, level: usize) -> Result<HashMap<String, FileInfo>, Error> {
        let mut result = HashMap::new();
        if level >= self.levels.len() {
            return Err(format!("Level {} does not exist", level).into());
        }

        for file in &self.levels[level].files {
            let info = FileInfo {
                hash: file
                    .hash
                    .clone()
                    .unwrap_or_else(|| util::get_buffer_hash(&file.data)),
                size: file.data.len() as u64,
            };
            result.insert(file.name.clone(), info);
        }

        Ok(result)
    }

    pub fn get_num_files(&self, level: usize) -> Result<usize, Error> {
        if level >= self.levels.len() {
            return Err(format!("Level {} does not exist", level).into());
        }
        Ok(self.levels[level].files.len())
    }
}
