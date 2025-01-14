use std::{
    cell::RefCell,
    collections::HashMap,
    fs::File,
    io::{BufRead, BufReader, BufWriter, Read, Write},
    path::Path,
    rc::Rc,
};

use countio::Counter;
use lzma::LzmaReader;

use crate::{Error, FileInfo};

struct RefReader<R> {
    inner: Rc<RefCell<Counter<R>>>,
    buf: Vec<u8>,
}
impl<R> RefReader<R> {
    fn new(inner: R) -> Self {
        Self {
            inner: Rc::new(RefCell::new(Counter::new(inner))),
            buf: Vec::new(),
        }
    }

    fn bytes_read(&self) -> usize {
        self.inner.borrow().reader_bytes()
    }
}
impl<R> Clone for RefReader<R> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            buf: self.buf.clone(),
        }
    }
}
impl<R: Read> Read for RefReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.inner.borrow_mut().read(buf)
    }
}
impl<R: BufRead> BufRead for RefReader<R> {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        self.buf.clear();
        self.buf
            .extend_from_slice(self.inner.borrow_mut().fill_buf()?);
        Ok(&self.buf)
    }

    fn consume(&mut self, amt: usize) {
        self.inner.borrow_mut().consume(amt)
    }
}

fn read_u32<R: Read>(reader: &mut R) -> Result<u32, Error> {
    let mut buf = [0; 4];
    reader.read_exact(&mut buf)?;
    let val = u32::from_be_bytes(buf);
    Ok(val)
}

fn read_stringz<R: BufRead>(reader: &mut R) -> Result<String, Error> {
    let mut buf = Vec::new();
    reader.read_until(0, &mut buf)?;
    buf.pop(); // Remove the null terminator
    let string = String::from_utf8(buf)?;
    Ok(string)
}

fn read_padding<R: Read>(reader: &mut R, size: usize) -> Result<(), Error> {
    let mut buf = vec![0; size];
    reader.read_exact(&mut buf)?;
    let sum: u8 = buf.iter().sum();
    if sum != 0 {
        return Err(format!(
            "Expected {} bytes of padding, but got non-zero sum {}",
            size, sum
        )
        .into());
    }
    Ok(())
}

struct ChunkReader<R> {
    stream: Counter<BufReader<LzmaReader<RefReader<R>>>>,
    file_idx: u32,
    header: ChunkHeader,
}
impl<R> ChunkReader<R> {
    fn next_file(&self) -> Option<FileMetadata> {
        if self.file_idx >= self.header.num_files {
            return None;
        }
        Some(self.header.file_metadata[self.file_idx as usize].clone())
    }
}
impl<R: Read> ChunkReader<R> {
    fn new(stream: RefReader<R>) -> Result<Self, Error> {
        let mut stream = Counter::new(BufReader::new(LzmaReader::new_decompressor(stream)?));
        let header = ChunkHeader::read(&mut stream)?;
        dbg!(&header);

        Ok(Self {
            stream,
            file_idx: 0,
            header,
        })
    }

    fn read_next_file<W: Write>(&mut self, writer: &mut W) -> Result<(), Error> {
        let file_metadata = self.next_file().ok_or("No more files")?;

        // seek to start of file
        if self.stream.reader_bytes() > file_metadata.offset as usize {
            return Err(format!(
                "Already read past file {} offset @ {} (read {} bytes already)",
                file_metadata.name,
                file_metadata.offset,
                self.stream.reader_bytes()
            )
            .into());
        };
        let padding_size = file_metadata.offset as usize - self.stream.reader_bytes();
        if padding_size > 0 {
            // TODO figure out why this isn't all zeroes between mainData and sharedAssets
            if let Err(e) = read_padding(&mut self.stream, padding_size) {
                eprintln!(
                    "Error reading padding before file {}: {}",
                    file_metadata.name, e
                );
            }
        }

        // write file_metadata.size bytes to writer
        let mut buf = BufWriter::new(writer);
        std::io::copy(
            &mut self.stream.by_ref().take(file_metadata.size as u64),
            &mut buf,
        )?;

        self.file_idx += 1;
        Ok(())
    }
}

pub struct AssetBundleReader<R> {
    stream: RefReader<R>,
    chunk_idx: u32,
    header: AssetBundleHeader,
}
impl AssetBundleReader<BufReader<File>> {
    pub fn from_file<P: AsRef<Path>>(file_path: P) -> Result<Self, Error> {
        let metadata = std::fs::metadata(&file_path)?;
        if !metadata.is_file() {
            return Err(format!("{} is not a file", file_path.as_ref().display()).into());
        }
        let file_size = metadata.len() as u32;

        let file = File::open(&file_path)?;
        let mut stream = RefReader::new(BufReader::new(file));

        let header = AssetBundleHeader::read(&mut stream, file_size)?;
        dbg!(&header);

        if stream.bytes_read() > header.header_size as usize {
            return Err(format!(
                "Bad header size {} for {} (read {} bytes)",
                header.header_size,
                file_path.as_ref().display(),
                stream.bytes_read()
            )
            .into());
        }

        let padding_size = header.header_size as usize - stream.bytes_read();
        if padding_size > 0 {
            read_padding(&mut stream, padding_size)?;
        }
        assert!(stream.bytes_read() % 4 == 0);

        Ok(Self {
            stream,
            chunk_idx: 0,
            header,
        })
    }
}
impl<R> AssetBundleReader<R> {
    pub fn get_uncompressed_info(self) -> Result<HashMap<String, FileInfo>, Error> {
        // todo
        Ok(HashMap::new())
    }
}
impl<R: Read> AssetBundleReader<R> {
    pub fn extract_all_files<P: AsRef<Path>>(mut self, output_dir_path: P) -> Result<(), Error> {
        if std::fs::exists(&output_dir_path)? {
            let metadata = std::fs::metadata(&output_dir_path)?;
            if !metadata.is_dir() {
                return Err(
                    format!("{} is not a directory", output_dir_path.as_ref().display()).into(),
                );
            }
        } else {
            std::fs::create_dir_all(&output_dir_path)?;
        }

        while let Some(mut chunk_reader) = self.get_next_chunk()? {
            while let Some(file_metadata) = chunk_reader.next_file() {
                let file_path = output_dir_path.as_ref().join(file_metadata.name);
                let file = File::create(file_path)?;
                let mut file_writer = BufWriter::new(file);
                chunk_reader.read_next_file(&mut file_writer)?;
            }
        }

        Ok(())
    }

    fn get_next_chunk(&mut self) -> Result<Option<ChunkReader<R>>, Error> {
        if self.chunk_idx >= self.header.chunk_count {
            return Ok(None);
        }

        // seek forward to start of chunk
        let chunk_offset = self.header.header_size
            + self
                .header
                .chunk_metadata
                .iter()
                .take(self.chunk_idx as usize)
                .map(|cm| cm.compressed_size)
                .sum::<u32>();

        if self.stream.bytes_read() > chunk_offset as usize {
            return Err(format!(
                "Already read past chunk {} offset @ {} (read {} bytes already)",
                self.chunk_idx,
                chunk_offset,
                self.stream.bytes_read()
            )
            .into());
        }
        let padding_size = chunk_offset as usize - self.stream.bytes_read();
        if padding_size > 0 {
            read_padding(&mut self.stream, padding_size)?;
        }

        let chunk_reader = ChunkReader::new(self.stream.clone())?;
        self.chunk_idx += 1;
        Ok(Some(chunk_reader))
    }
}

#[derive(Debug, Clone)]
struct FileMetadata {
    name: String,
    offset: u32, // within the uncompressed chunk, 4-byte aligned
    size: u32,
}

#[derive(Debug)]
struct ChunkHeader {
    num_files: u32,
    file_metadata: Vec<FileMetadata>,
}
impl ChunkHeader {
    fn read<R: Read + BufRead>(reader: &mut R) -> Result<Self, Error> {
        let num_files = read_u32(reader)?;

        let mut file_metadata: Vec<FileMetadata> = Vec::new();
        for _ in 0..num_files {
            let name = read_stringz(reader)?;
            let offset = read_u32(reader)?;
            let size = read_u32(reader)?;

            if offset % 4 != 0 {
                return Err(format!(
                    "Chunk offset {} is not 4-byte aligned for file {}",
                    offset, name
                )
                .into());
            }

            let entry = FileMetadata { name, offset, size };

            // in valid bundles, the offsets are always in increasing order.
            // this enables streaming optimizations
            if file_metadata.last().is_some_and(|fm| fm.offset > offset) {
                return Err(format!(
                    "File offsets are not in increasing order:\nfile[{}] = {:?}\nfile[{}] = {:?}",
                    file_metadata.len() - 1,
                    file_metadata.last().unwrap(),
                    file_metadata.len(),
                    &entry,
                )
                .into());
            }

            file_metadata.push(entry);
        }

        Ok(Self {
            num_files,
            file_metadata,
        })
    }
}

#[derive(Debug)]
struct ChunkMetadata {
    compressed_size: u32,
    uncompressed_size: u32,
}

const EXPECTED_SIGNATURE: &str = "UnityWeb";
const EXPECTED_STREAM_VERSION: u32 = 2;
const EXPECTED_PLAYER_VERSION: &str = "fusion-2.x.x";
const EXPECTED_ENGINE_VERSION_BASE: &str = "2.5";

#[derive(Debug)]
struct AssetBundleHeader {
    signature: String,
    stream_version: u32,
    player_version: String,
    engine_version: String,
    minimum_streamed_bytes: u32,
    header_size: u32, // 4-byte aligned
    chunks_to_stream: u32,
    chunk_count: u32,
    chunk_metadata: Vec<ChunkMetadata>,
    total_bytes: u32,
}
impl AssetBundleHeader {
    fn read<R: BufRead>(reader: &mut R, file_size: u32) -> Result<Self, Error> {
        let signature = read_stringz(reader)?;
        if signature != EXPECTED_SIGNATURE {
            return Err(format!(
                "Unexpected signature {}, expected {}",
                signature, EXPECTED_SIGNATURE
            )
            .into());
        }

        let stream_version = read_u32(reader)?;
        if stream_version != EXPECTED_STREAM_VERSION {
            return Err(format!(
                "Unexpected stream version {}, expected {}",
                stream_version, EXPECTED_STREAM_VERSION
            )
            .into());
        }

        let player_version = read_stringz(reader)?;
        if player_version != EXPECTED_PLAYER_VERSION {
            return Err(format!(
                "Unexpected player version {}, expected {}",
                player_version, EXPECTED_PLAYER_VERSION
            )
            .into());
        }

        let engine_version = read_stringz(reader)?;
        if !engine_version.starts_with(EXPECTED_ENGINE_VERSION_BASE) {
            return Err(format!(
                "Unexpected engine version {}, expected {}*",
                engine_version, EXPECTED_ENGINE_VERSION_BASE
            )
            .into());
        }

        let minimum_streamed_bytes = read_u32(reader)?;
        let header_size = read_u32(reader)?;
        if header_size % 4 != 0 {
            return Err(format!("Total header size {} is not 4-byte aligned", header_size).into());
        }

        let chunks_to_stream = read_u32(reader)?;
        if chunks_to_stream != 1 {
            return Err(format!(
                "Expected only one chunk to stream, but got {}",
                chunks_to_stream
            )
            .into());
        }

        let chunk_count = read_u32(reader)?;

        if chunk_count < chunks_to_stream {
            return Err(format!(
                "Chunk count {} is less than chunks to stream {}",
                chunk_count, chunks_to_stream
            )
            .into());
        }

        let mut chunk_metadata = Vec::new();
        for _ in 0..chunk_count {
            let compressed_size = read_u32(reader)?;
            let uncompressed_size = read_u32(reader)?;
            chunk_metadata.push(ChunkMetadata {
                compressed_size,
                uncompressed_size,
            });
        }

        let total_bytes = read_u32(reader)?;
        if total_bytes != file_size {
            return Err(format!(
                "Total bytes {} does not match file size {}",
                total_bytes, file_size
            )
            .into());
        }

        Ok(Self {
            signature,
            stream_version,
            player_version,
            engine_version,
            minimum_streamed_bytes,
            header_size,
            chunks_to_stream,
            chunk_count,
            chunk_metadata,
            total_bytes,
        })
    }
}
