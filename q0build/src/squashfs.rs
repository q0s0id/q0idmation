//! A squashfs 4.0 writer for AppImages: directories and regular files,
//! every block stored uncompressed, no fragments, one owner (root).
//!
//! The superblock names zstd, which the AppImage runtime supports, but no
//! block is compressed, so no decompressor ever runs. The image is larger
//! than a compressed one; the q0s movie inside is packed already.

use std::collections::BTreeMap;

const BLOCK_SIZE: usize = 128 * 1024;
const BLOCK_LOG: u16 = 17;
const METADATA_BLOCK: usize = 8192;
const COMPRESSION_ZSTD: u16 = 6;
const UNCOMPRESSED_DATA_BLOCK: u32 = 1 << 24;
const UNCOMPRESSED_METADATA: u16 = 0x8000;
const NO_TABLE: u64 = u64::MAX;
// Uncompressed inodes, data, fragments and ids; no fragments; no xattrs.
const FLAGS: u16 = 0x0001 | 0x0002 | 0x0008 | 0x0010 | 0x0200 | 0x0800;
const DIR_INODE: u16 = 1;
const FILE_INODE: u16 = 2;

enum Node {
    Dir(BTreeMap<String, Node>),
    File { data: Vec<u8>, mode: u16 },
}

/// The files of an image: paths use `/`, folders are made as needed.
#[derive(Default)]
pub struct Image {
    root: BTreeMap<String, Node>,
}

impl Image {
    pub fn add(&mut self, path: &str, data: Vec<u8>, mode: u16) {
        let mut parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
        let name = parts.pop().expect("a file has a name").to_string();
        let mut dir = &mut self.root;
        for part in parts {
            let node = dir
                .entry(part.to_string())
                .or_insert_with(|| Node::Dir(BTreeMap::new()));
            dir = match node {
                Node::Dir(children) => children,
                Node::File { .. } => panic!("{part} is a file and a folder"),
            };
        }
        dir.insert(name, Node::File { data, mode });
    }

    /// The image's bytes.
    pub fn write(&self, mtime: u32) -> Vec<u8> {
        let mut out = vec![0_u8; 96];
        let mut writer = Writer {
            inodes: Vec::new(),
            listings: Vec::new(),
            next_inode: 1,
            mtime,
        };
        // File data first, each file's blocks one after another.
        let mut placed: Vec<(u32, Vec<u32>)> = Vec::new();
        place_data(&self.root, &mut out, &mut placed);
        let mut placed = placed.into_iter();
        let (root_ref, _, root_at) = writer.write_dir(&self.root, &mut placed);
        let inode_count = writer.next_inode - 1;
        // The root's parent is one past the last inode, as mksquashfs writes.
        writer.inodes[root_at + 28..root_at + 32].copy_from_slice(&(inode_count + 1).to_le_bytes());

        let inode_table_start = out.len() as u64;
        out.extend(metadata_blocks(&writer.inodes));
        let directory_table_start = out.len() as u64;
        out.extend(metadata_blocks(&writer.listings));
        let id_block = out.len() as u64;
        out.extend(metadata_blocks(&0_u32.to_le_bytes()));
        let id_table_start = out.len() as u64;
        out.extend_from_slice(&id_block.to_le_bytes());
        let bytes_used = out.len() as u64;
        // Readers expect the image padded to 4 KiB.
        out.resize(out.len().div_ceil(4096) * 4096, 0);

        let mut superblock = Vec::with_capacity(96);
        superblock.extend_from_slice(&0x7371_7368_u32.to_le_bytes());
        superblock.extend_from_slice(&inode_count.to_le_bytes());
        superblock.extend_from_slice(&mtime.to_le_bytes());
        superblock.extend_from_slice(&(BLOCK_SIZE as u32).to_le_bytes());
        superblock.extend_from_slice(&0_u32.to_le_bytes());
        superblock.extend_from_slice(&COMPRESSION_ZSTD.to_le_bytes());
        superblock.extend_from_slice(&BLOCK_LOG.to_le_bytes());
        superblock.extend_from_slice(&FLAGS.to_le_bytes());
        superblock.extend_from_slice(&1_u16.to_le_bytes());
        superblock.extend_from_slice(&4_u16.to_le_bytes());
        superblock.extend_from_slice(&0_u16.to_le_bytes());
        superblock.extend_from_slice(&root_ref.to_le_bytes());
        superblock.extend_from_slice(&bytes_used.to_le_bytes());
        superblock.extend_from_slice(&id_table_start.to_le_bytes());
        superblock.extend_from_slice(&NO_TABLE.to_le_bytes());
        superblock.extend_from_slice(&inode_table_start.to_le_bytes());
        superblock.extend_from_slice(&directory_table_start.to_le_bytes());
        superblock.extend_from_slice(&NO_TABLE.to_le_bytes());
        superblock.extend_from_slice(&NO_TABLE.to_le_bytes());
        out[..96].copy_from_slice(&superblock);
        out
    }
}

/// Every file's data blocks, in the order `Writer::write_dir` visits files.
fn place_data(dir: &BTreeMap<String, Node>, out: &mut Vec<u8>, placed: &mut Vec<(u32, Vec<u32>)>) {
    for node in dir.values() {
        match node {
            Node::Dir(children) => place_data(children, out, placed),
            Node::File { data, .. } => {
                let start = out.len() as u32;
                let sizes = data
                    .chunks(BLOCK_SIZE)
                    .map(|block| {
                        out.extend_from_slice(block);
                        block.len() as u32 | UNCOMPRESSED_DATA_BLOCK
                    })
                    .collect();
                placed.push((start, sizes));
            }
        }
    }
}

/// Metadata as stored: 8 KiB pieces, each behind a two-byte header.
fn metadata_blocks(stream: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(stream.len() + stream.len() / METADATA_BLOCK * 2 + 2);
    for block in stream.chunks(METADATA_BLOCK) {
        out.extend_from_slice(&(block.len() as u16 | UNCOMPRESSED_METADATA).to_le_bytes());
        out.extend_from_slice(block);
    }
    out
}

/// Where something starts in a metadata stream: the stored block's offset
/// in its table, and the offset inside that block.
fn position(stream_offset: usize) -> (u32, u16) {
    let block = stream_offset / METADATA_BLOCK;
    (
        (block * (METADATA_BLOCK + 2)) as u32,
        (stream_offset % METADATA_BLOCK) as u16,
    )
}

struct Writer {
    inodes: Vec<u8>,
    listings: Vec<u8>,
    next_inode: u32,
    mtime: u32,
}

impl Writer {
    fn header(&mut self, kind: u16, mode: u16) -> u32 {
        let number = self.next_inode;
        self.next_inode += 1;
        for value in [kind, mode, 0, 0] {
            self.inodes.extend_from_slice(&value.to_le_bytes());
        }
        self.inodes.extend_from_slice(&self.mtime.to_le_bytes());
        self.inodes.extend_from_slice(&number.to_le_bytes());
        number
    }

    /// Writes a folder's children, then its listing and inode: children
    /// first, so the listing knows where they are. Returns the folder's
    /// inode reference, number and place in the inode stream; its parent
    /// field is filled in by whoever writes the parent.
    fn write_dir(
        &mut self,
        dir: &BTreeMap<String, Node>,
        placed: &mut impl Iterator<Item = (u32, Vec<u32>)>,
    ) -> (u64, u32, usize) {
        // (name, kind, inode position in the stream, inode number)
        let mut entries: Vec<(&str, u16, usize, u32)> = Vec::new();
        let mut subdirs: u32 = 0;
        for (name, node) in dir {
            match node {
                Node::Dir(children) => {
                    let (_, number, at) = self.write_dir(children, placed);
                    entries.push((name, DIR_INODE, at, number));
                    subdirs += 1;
                }
                Node::File { data, mode } => {
                    let (start, sizes) = placed.next().expect("placed in the same order");
                    let at = self.inodes.len();
                    let number = self.header(FILE_INODE, *mode);
                    self.inodes.extend_from_slice(&start.to_le_bytes());
                    self.inodes.extend_from_slice(&u32::MAX.to_le_bytes());
                    self.inodes.extend_from_slice(&0_u32.to_le_bytes());
                    self.inodes
                        .extend_from_slice(&(data.len() as u32).to_le_bytes());
                    for size in sizes {
                        self.inodes.extend_from_slice(&size.to_le_bytes());
                    }
                    entries.push((name, FILE_INODE, at, number));
                }
            }
        }
        // The listing: runs of entries whose inodes share a metadata block.
        let listing_start = self.listings.len();
        let mut run_start = 0;
        while run_start < entries.len() {
            let block = position(entries[run_start].2).0;
            let base = entries[run_start].3;
            let mut run_end = run_start + 1;
            while run_end < entries.len()
                && run_end - run_start < 256
                && position(entries[run_end].2).0 == block
                && (i64::from(entries[run_end].3) - i64::from(base)).abs() < 32767
            {
                run_end += 1;
            }
            self.listings
                .extend_from_slice(&((run_end - run_start - 1) as u32).to_le_bytes());
            self.listings.extend_from_slice(&block.to_le_bytes());
            self.listings.extend_from_slice(&base.to_le_bytes());
            for (name, kind, at, number) in &entries[run_start..run_end] {
                self.listings
                    .extend_from_slice(&position(*at).1.to_le_bytes());
                self.listings
                    .extend_from_slice(&((*number as i64 - base as i64) as i16).to_le_bytes());
                self.listings.extend_from_slice(&kind.to_le_bytes());
                self.listings
                    .extend_from_slice(&((name.len() - 1) as u16).to_le_bytes());
                self.listings.extend_from_slice(name.as_bytes());
            }
            run_start = run_end;
        }
        let listing_size = self.listings.len() - listing_start;
        let (listing_block, listing_offset) = position(listing_start);

        let at = self.inodes.len();
        let number = self.header(DIR_INODE, 0o755);
        self.inodes.extend_from_slice(&listing_block.to_le_bytes());
        self.inodes.extend_from_slice(&(2 + subdirs).to_le_bytes());
        self.inodes
            .extend_from_slice(&((listing_size + 3) as u16).to_le_bytes());
        self.inodes.extend_from_slice(&listing_offset.to_le_bytes());
        self.inodes.extend_from_slice(&0_u32.to_le_bytes());
        // Children folders point back at this one.
        for (_, kind, child_at, _) in &entries {
            if *kind == DIR_INODE {
                let parent_field = child_at + 28;
                self.inodes[parent_field..parent_field + 4].copy_from_slice(&number.to_le_bytes());
            }
        }
        let (block, offset) = position(at);
        (u64::from(block) << 16 | u64::from(offset), number, at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads one file back the way a squashfs reader walks the image, for
    /// files this writer made (uncompressed, unfragmented).
    fn read_file(image: &[u8], path: &str) -> Option<Vec<u8>> {
        let u16_at = |at: usize| u16::from_le_bytes(image[at..at + 2].try_into().unwrap());
        let u32_at = |at: usize| u32::from_le_bytes(image[at..at + 4].try_into().unwrap());
        let u64_at = |at: usize| u64::from_le_bytes(image[at..at + 8].try_into().unwrap());
        let inode_table = u64_at(64) as usize;
        let dir_table = u64_at(72) as usize;
        let unpack = |table: usize, end: usize| -> Vec<u8> {
            let mut stream = Vec::new();
            let mut at = table;
            while at < end {
                let header = u16_at(at);
                let length = usize::from(header & 0x7FFF);
                stream.extend_from_slice(&image[at + 2..at + 2 + length]);
                at += 2 + length;
            }
            stream
        };
        let inodes = unpack(inode_table, dir_table);
        let listings = unpack(dir_table, u64_at(48) as usize - 6);
        let stream_at = |reference: u64| {
            (reference >> 16) as usize / (METADATA_BLOCK + 2) * METADATA_BLOCK
                + (reference & 0xFFFF) as usize
        };
        let mut inode = stream_at(u64_at(32));
        for part in path.split('/') {
            let start_block =
                u32::from_le_bytes(inodes[inode + 16..inode + 20].try_into().unwrap()) as usize;
            let size =
                u16::from_le_bytes(inodes[inode + 24..inode + 26].try_into().unwrap()) as usize - 3;
            let offset =
                u16::from_le_bytes(inodes[inode + 26..inode + 28].try_into().unwrap()) as usize;
            let mut at = start_block / (METADATA_BLOCK + 2) * METADATA_BLOCK + offset;
            let end = at + size;
            let mut found = None;
            while at < end {
                let count = u32::from_le_bytes(listings[at..at + 4].try_into().unwrap()) + 1;
                let block = u32::from_le_bytes(listings[at + 4..at + 8].try_into().unwrap());
                at += 12;
                for _ in 0..count {
                    let entry_offset = u16::from_le_bytes(listings[at..at + 2].try_into().unwrap());
                    let name_size = u16::from_le_bytes(listings[at + 6..at + 8].try_into().unwrap())
                        as usize
                        + 1;
                    let name = &listings[at + 8..at + 8 + name_size];
                    if name == part.as_bytes() {
                        found = Some(stream_at(u64::from(block) << 16 | u64::from(entry_offset)));
                    }
                    at += 8 + name_size;
                }
            }
            inode = found?;
        }
        assert_eq!(
            u16::from_le_bytes(inodes[inode..inode + 2].try_into().unwrap()),
            FILE_INODE
        );
        let start = u32::from_le_bytes(inodes[inode + 16..inode + 20].try_into().unwrap()) as usize;
        let size = u32::from_le_bytes(inodes[inode + 28..inode + 32].try_into().unwrap()) as usize;
        let _ = u32_at;
        Some(image[start..start + size].to_vec())
    }

    #[test]
    fn files_read_back_from_their_folders() {
        let mut image = Image::default();
        let big: Vec<u8> = (0..300_000_u32).map(|i| (i % 251) as u8).collect();
        image.add("AppRun", b"#!/bin/sh\n".to_vec(), 0o755);
        image.add("usr/bin/game", big.clone(), 0o755);
        image.add(
            "usr/share/applications/game.desktop",
            b"[Desktop Entry]\n".to_vec(),
            0o644,
        );
        image.add(".DirIcon", vec![1, 2, 3], 0o644);
        let bytes = image.write(1_700_000_000);
        assert_eq!(&bytes[..4], b"hsqs");
        assert_eq!(bytes.len() % 4096, 0);
        assert_eq!(read_file(&bytes, "AppRun").unwrap(), b"#!/bin/sh\n");
        assert_eq!(read_file(&bytes, "usr/bin/game").unwrap(), big);
        assert_eq!(
            read_file(&bytes, "usr/share/applications/game.desktop").unwrap(),
            b"[Desktop Entry]\n"
        );
        assert_eq!(read_file(&bytes, ".DirIcon").unwrap(), vec![1, 2, 3]);
        assert!(read_file(&bytes, "usr/bin/missing").is_none());
        // Inode count: 4 files + usr, bin, share, applications + root.
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 9);
    }

    #[test]
    fn many_entries_split_across_metadata_blocks() {
        let mut image = Image::default();
        for index in 0..600 {
            image.add(
                &format!("many/file-{index:04}.txt"),
                format!("file {index}").into_bytes(),
                0o644,
            );
        }
        let bytes = image.write(0);
        for index in [0, 299, 599] {
            assert_eq!(
                read_file(&bytes, &format!("many/file-{index:04}.txt")).unwrap(),
                format!("file {index}").into_bytes()
            );
        }
    }
}
