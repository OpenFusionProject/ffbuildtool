use std::{
    collections::HashMap,
    fs::File,
    io::{BufRead, BufReader, Read},
    path::Path,
};

use crate::{Error, FileInfo};

const EXPECTED_SIGNATURE: &str = "UnityWeb";
const EXPECTED_STREAM_VERSION: u32 = 2;
const EXPECTED_PLAYER_VERSION: &str = "fusion-2.x.x";
const EXPECTED_ENGINE_VERSION_BASE: &str = "2.5";

fn read_u32<R: Read>(reader: &mut R, counter: &mut usize) -> Result<u32, Error> {
    let mut buf = [0; 4];
    reader.read_exact(&mut buf)?;
    let val = u32::from_be_bytes(buf);
    *counter += 4;
    Ok(val)
}

fn read_stringz<R: BufRead>(reader: &mut R, counter: &mut usize) -> Result<String, Error> {
    let mut buf = Vec::new();
    let bytes_read = reader.read_until(0, &mut buf)?;
    buf.pop(); // Remove the null terminator
    let string = String::from_utf8(buf)?;
    *counter += bytes_read;
    Ok(string)
}

#[derive(Debug)]
struct ChunkMetadata {
    compressed_size: u32,   // 4-byte aligned?
    uncompressed_size: u32, // 4-byte aligned?
}

#[derive(Debug)]
struct AssetBundleHeader {
    signature: String,
    stream_version: u32,
    player_version: String,
    engine_version: String,
    minimum_streamed_bytes: u32,
    total_header_size: u32, // 4-byte aligned
    chunks_to_stream: u32,
    chunk_count: u32,
    chunk_metadata: Vec<ChunkMetadata>,
    total_bytes: u32,
}
impl AssetBundleHeader {
    fn read<R: BufRead>(reader: &mut R, file_size: u32) -> Result<Self, Error> {
        let mut counter = 0;

        let signature = read_stringz(reader, &mut counter)?;
        if signature != EXPECTED_SIGNATURE {
            return Err(format!(
                "Unexpected signature {}, expected {}",
                signature, EXPECTED_SIGNATURE
            )
            .into());
        }

        let stream_version = read_u32(reader, &mut counter)?;
        if stream_version != EXPECTED_STREAM_VERSION {
            return Err(format!(
                "Unexpected stream version {}, expected {}",
                stream_version, EXPECTED_STREAM_VERSION
            )
            .into());
        }

        let player_version = read_stringz(reader, &mut counter)?;
        if player_version != EXPECTED_PLAYER_VERSION {
            return Err(format!(
                "Unexpected player version {}, expected {}",
                player_version, EXPECTED_PLAYER_VERSION
            )
            .into());
        }

        let engine_version = read_stringz(reader, &mut counter)?;
        if !engine_version.starts_with(EXPECTED_ENGINE_VERSION_BASE) {
            return Err(format!(
                "Unexpected engine version {}, expected {}*",
                engine_version, EXPECTED_ENGINE_VERSION_BASE
            )
            .into());
        }

        let minimum_streamed_bytes = read_u32(reader, &mut counter)?;
        let total_header_size = read_u32(reader, &mut counter)?;
        if total_header_size % 4 != 0 {
            return Err(format!(
                "Total header size {} is not 4-byte aligned",
                total_header_size
            )
            .into());
        }

        let chunks_to_stream = read_u32(reader, &mut counter)?;
        if chunks_to_stream != 1 {
            return Err(format!(
                "Expected only one chunk to stream, but got {}",
                chunks_to_stream
            )
            .into());
        }

        let chunk_count = read_u32(reader, &mut counter)?;
        if chunk_count != 1 {
            return Err(format!("Expected only one chunk, but got {}", chunk_count).into());
        }

        if chunk_count < chunks_to_stream {
            return Err(format!(
                "Chunk count {} is less than chunks to stream {}",
                chunk_count, chunks_to_stream
            )
            .into());
        }

        let mut chunk_metadata = Vec::new();
        for _ in 0..chunk_count {
            let compressed_size = read_u32(reader, &mut counter)?;
            let uncompressed_size = read_u32(reader, &mut counter)?;
            chunk_metadata.push(ChunkMetadata {
                compressed_size,
                uncompressed_size,
            });
        }

        let total_bytes = read_u32(reader, &mut counter)?;
        if total_bytes != file_size {
            return Err(format!(
                "Total bytes {} does not match file size {}",
                total_bytes, file_size
            )
            .into());
        }

        let bytes_left = total_header_size - counter as u32;
        if bytes_left > 0 {
            let mut buf = Vec::with_capacity(bytes_left as usize);
            reader.read_exact(&mut buf)?;
            let sum: u8 = buf.iter().sum();
            if sum != 0 {
                return Err(format!(
                    "Expected {} bytes of padding, but got non-zero sum {}",
                    bytes_left, sum
                )
                .into());
            }
        }

        Ok(Self {
            signature,
            stream_version,
            player_version,
            engine_version,
            minimum_streamed_bytes,
            total_header_size,
            chunks_to_stream,
            chunk_count,
            chunk_metadata,
            total_bytes,
        })
    }

    fn get_total_compressed_size(&self) -> u32 {
        self.chunk_metadata.iter().map(|x| x.compressed_size).sum()
    }

    fn get_total_uncompressed_size(&self) -> u32 {
        self.chunk_metadata
            .iter()
            .map(|x| x.uncompressed_size)
            .sum()
    }
}

#[derive(Debug)]
pub struct AssetBundleReader {
    header: AssetBundleHeader,
    reader: BufReader<File>,
    chunk_index: u32,
    current_chunk: Vec<u8>,
}
impl AssetBundleReader {
    pub fn from_file<P: AsRef<Path>>(file_path: P) -> Result<Self, Error> {
        let metadata = std::fs::metadata(&file_path)?;
        if !metadata.is_file() {
            return Err(format!("{} is not a file", file_path.as_ref().display()).into());
        }

        let file = File::open(&file_path)?;
        let mut reader = BufReader::new(file);

        let file_size = metadata.len() as u32;
        let header = AssetBundleHeader::read(&mut reader, file_size)?;
        dbg!(&header);

        let current_chunk = Vec::with_capacity(header.get_total_uncompressed_size() as usize);

        Ok(Self {
            header,
            reader,
            chunk_index: 0,
            current_chunk,
        })
    }

    pub fn get_uncompressed_info(self) -> Result<HashMap<String, FileInfo>, Error> {
        // todo
        Ok(HashMap::new())
    }

    pub fn extract_all_files<P: AsRef<Path>>(self, output_dir_path: P) -> Result<(), Error> {
        let metadata = std::fs::metadata(&output_dir_path)?;
        if !metadata.is_dir() {
            return Err(
                format!("{} is not a directory", output_dir_path.as_ref().display()).into(),
            );
        }
        // todo
        Ok(())
    }

    fn read_in_next_chunk(&mut self) -> Result<(), Error> {
        if self.chunk_index >= self.header.chunk_count {
            return Err("No more chunks to read".into());
        }

        // todo

        Ok(())
    }
}
