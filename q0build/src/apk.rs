//! Android: q0player's APK made into the app's own. The bundle goes into
//! `assets/qir/app.qir`; the binary manifest gets the app's package, name
//! and version; the launcher icons are swapped; and the result is aligned
//! and signed (APK Signature Scheme v2, ECDSA P-256) with a key Q0Build
//! keeps for this computer, so later builds install as updates.

use std::io::{Cursor, Read, Write};
use std::path::PathBuf;

use crate::icons::AppIcon;

/// What changes in the APK.
pub struct ApkPlan<'a> {
    pub manifest: &'a q0qir::Manifest,
    pub bundle: &'a [u8],
    pub icon: &'a AppIcon,
    pub drop_open_with: bool,
}

pub fn build(template: &[u8], plan: &ApkPlan, signer: &Signer) -> Result<Vec<u8>, String> {
    let zipped = repack(template, plan)?;
    sign_v2(zipped, signer)
}

// ---------------------------------------------------------------- zip

fn repack(template: &[u8], plan: &ApkPlan) -> Result<Vec<u8>, String> {
    let zip_error = |error: zip::result::ZipError| format!("the APK template: {error}");
    let mut reader = zip::ZipArchive::new(Cursor::new(template)).map_err(zip_error)?;
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let stored =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let deflated =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut manifest_seen = false;
    for index in 0..reader.len() {
        let name = reader
            .by_index_raw(index)
            .map_err(zip_error)?
            .name()
            .to_string();
        let upper = name.to_ascii_uppercase();
        // The old signature goes; this APK is signed anew below.
        if upper.starts_with("META-INF/")
            && (upper.ends_with(".SF")
                || upper.ends_with(".RSA")
                || upper.ends_with(".EC")
                || upper.ends_with(".DSA")
                || upper == "META-INF/MANIFEST.MF")
        {
            continue;
        }
        // A template that already carries a bundle loses it.
        if name == format!("assets/{}", q0qir::ANDROID_ASSET) {
            continue;
        }
        if name == "AndroidManifest.xml" {
            let mut bytes = Vec::new();
            reader
                .by_index(index)
                .map_err(zip_error)?
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            let patched = patch_manifest(&bytes, plan)?;
            writer.start_file(name, deflated).map_err(zip_error)?;
            writer.write_all(&patched).map_err(|e| e.to_string())?;
            manifest_seen = true;
            continue;
        }
        if is_launcher_icon(&name) {
            let mut bytes = Vec::new();
            reader
                .by_index(index)
                .map_err(zip_error)?
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            let side = image::load_from_memory(&bytes)
                .map(|old| old.width())
                .unwrap_or(192);
            writer
                .start_file_aligned(name, stored, 4)
                .map_err(zip_error)?;
            writer
                .write_all(&plan.icon.png(side))
                .map_err(|e| e.to_string())?;
            continue;
        }
        let compressed = reader.by_index_raw(index).map_err(zip_error)?.compression()
            != zip::CompressionMethod::Stored;
        if compressed {
            let file = reader.by_index_raw(index).map_err(zip_error)?;
            writer.raw_copy_file(file).map_err(zip_error)?;
            continue;
        }
        // Stored entries are read in place by Android: native libraries on
        // a 16 KiB page, everything else (resources.arsc) on four bytes.
        let mut bytes = Vec::new();
        reader
            .by_index(index)
            .map_err(zip_error)?
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        let align = if name.ends_with(".so") { 16384 } else { 4 };
        writer
            .start_file_aligned(name, stored, align)
            .map_err(zip_error)?;
        writer.write_all(&bytes).map_err(|e| e.to_string())?;
    }
    if !manifest_seen {
        return Err("the APK template has no AndroidManifest.xml".into());
    }
    writer
        .start_file_aligned(format!("assets/{}", q0qir::ANDROID_ASSET), stored, 4)
        .map_err(zip_error)?;
    writer.write_all(plan.bundle).map_err(|e| e.to_string())?;
    Ok(writer.finish().map_err(zip_error)?.into_inner())
}

fn is_launcher_icon(name: &str) -> bool {
    name.starts_with("res/mipmap")
        && name
            .rsplit('/')
            .next()
            .is_some_and(|file| file.starts_with("ic_launcher") && file.ends_with(".png"))
}

// ---------------------------------------------------------------- manifest

const RES_STRING_POOL: u16 = 0x0001;
const RES_XML_START_ELEMENT: u16 = 0x0102;
const RES_XML_END_ELEMENT: u16 = 0x0103;
const UTF8_FLAG: u32 = 0x100;
const TYPE_STRING: u8 = 0x03;
const TYPE_INT_DEC: u8 = 0x10;
const NO_STRING: u32 = 0xFFFF_FFFF;

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

/// A binary XML file: its string pool and every other chunk as it was.
struct Axml {
    strings: Vec<String>,
    utf8: bool,
    chunks: Vec<Vec<u8>>,
}

impl Axml {
    fn parse(bytes: &[u8]) -> Result<Self, String> {
        let bad = || "AndroidManifest.xml is not binary XML Q0Build understands".to_string();
        if bytes.len() < 8 || u16_at(bytes, 0) != 0x0003 {
            return Err(bad());
        }
        let mut at = usize::from(u16_at(bytes, 2));
        let mut strings = None;
        let mut utf8 = false;
        let mut chunks = Vec::new();
        while at + 8 <= bytes.len() {
            let kind = u16_at(bytes, at);
            let size = u32_at(bytes, at + 4) as usize;
            if size < 8 || at + size > bytes.len() {
                return Err(bad());
            }
            let chunk = &bytes[at..at + size];
            if kind == RES_STRING_POOL && strings.is_none() {
                let count = u32_at(chunk, 8) as usize;
                let styles = u32_at(chunk, 12);
                let flags = u32_at(chunk, 16);
                let start = u32_at(chunk, 20) as usize;
                if styles != 0 {
                    return Err(
                        "the manifest's string pool has styles, which Q0Build does not rewrite"
                            .into(),
                    );
                }
                utf8 = flags & UTF8_FLAG != 0;
                let mut list = Vec::with_capacity(count);
                for index in 0..count {
                    let offset = start + u32_at(chunk, 28 + index * 4) as usize;
                    list.push(read_pool_string(chunk, offset, utf8).ok_or_else(bad)?);
                }
                strings = Some(list);
            } else {
                chunks.push(chunk.to_vec());
            }
            at += size;
        }
        Ok(Self {
            strings: strings.ok_or_else(bad)?,
            utf8,
            chunks,
        })
    }

    fn intern(&mut self, text: &str) -> u32 {
        if let Some(index) = self.strings.iter().position(|existing| existing == text) {
            return index as u32;
        }
        // New strings go last: the resource map covers the first ones.
        self.strings.push(text.to_string());
        (self.strings.len() - 1) as u32
    }

    fn text(&self, index: u32) -> &str {
        self.strings.get(index as usize).map_or("", String::as_str)
    }

    fn write(&self) -> Vec<u8> {
        let mut data = Vec::new();
        let mut offsets = Vec::with_capacity(self.strings.len());
        for text in &self.strings {
            offsets.push(data.len() as u32);
            if self.utf8 {
                let units = text.encode_utf16().count();
                push_utf8_length(&mut data, units);
                push_utf8_length(&mut data, text.len());
                data.extend_from_slice(text.as_bytes());
                data.push(0);
            } else {
                let units: Vec<u16> = text.encode_utf16().collect();
                if units.len() > 0x7FFF {
                    data.extend_from_slice(&((0x8000 | (units.len() >> 16)) as u16).to_le_bytes());
                    data.extend_from_slice(&((units.len() & 0xFFFF) as u16).to_le_bytes());
                } else {
                    data.extend_from_slice(&(units.len() as u16).to_le_bytes());
                }
                for unit in units {
                    data.extend_from_slice(&unit.to_le_bytes());
                }
                data.extend_from_slice(&[0, 0]);
            }
        }
        while !data.len().is_multiple_of(4) {
            data.push(0);
        }
        let count = self.strings.len() as u32;
        let strings_start = 28 + 4 * count;
        let pool_size = strings_start + data.len() as u32;
        let mut pool = Vec::with_capacity(pool_size as usize);
        pool.extend_from_slice(&RES_STRING_POOL.to_le_bytes());
        pool.extend_from_slice(&28_u16.to_le_bytes());
        pool.extend_from_slice(&pool_size.to_le_bytes());
        pool.extend_from_slice(&count.to_le_bytes());
        pool.extend_from_slice(&0_u32.to_le_bytes());
        pool.extend_from_slice(&(if self.utf8 { UTF8_FLAG } else { 0 }).to_le_bytes());
        pool.extend_from_slice(&strings_start.to_le_bytes());
        pool.extend_from_slice(&0_u32.to_le_bytes());
        for offset in offsets {
            pool.extend_from_slice(&offset.to_le_bytes());
        }
        pool.extend_from_slice(&data);

        let total = 8 + pool.len() + self.chunks.iter().map(Vec::len).sum::<usize>();
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(&0x0003_u16.to_le_bytes());
        out.extend_from_slice(&8_u16.to_le_bytes());
        out.extend_from_slice(&(total as u32).to_le_bytes());
        out.extend_from_slice(&pool);
        for chunk in &self.chunks {
            out.extend_from_slice(chunk);
        }
        out
    }
}

fn push_utf8_length(data: &mut Vec<u8>, length: usize) {
    if length > 0x7F {
        data.push(0x80 | ((length >> 8) & 0x7F) as u8);
        data.push((length & 0xFF) as u8);
    } else {
        data.push(length as u8);
    }
}

fn read_pool_string(chunk: &[u8], at: usize, utf8: bool) -> Option<String> {
    if utf8 {
        let mut at = at;
        let skip_length = |at: &mut usize| {
            let first = *chunk.get(*at)?;
            *at += if first & 0x80 != 0 { 2 } else { 1 };
            Some(())
        };
        skip_length(&mut at)?;
        let first = *chunk.get(at)? as usize;
        let length = if first & 0x80 != 0 {
            let second = *chunk.get(at + 1)? as usize;
            at += 2;
            ((first & 0x7F) << 8) | second
        } else {
            at += 1;
            first
        };
        String::from_utf8(chunk.get(at..at + length)?.to_vec()).ok()
    } else {
        let first = u16::from_le_bytes([*chunk.get(at)?, *chunk.get(at + 1)?]) as usize;
        let (length, start) = if first & 0x8000 != 0 {
            let second = u16::from_le_bytes([*chunk.get(at + 2)?, *chunk.get(at + 3)?]) as usize;
            (((first & 0x7FFF) << 16) | second, at + 4)
        } else {
            (first, at + 2)
        };
        let units: Vec<u16> = chunk
            .get(start..start + length * 2)?
            .chunks(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        Some(String::from_utf16_lossy(&units))
    }
}

/// An element's name and its attributes' positions inside the chunk.
fn element_name(chunk: &[u8]) -> u32 {
    u32_at(chunk, 20)
}

fn attributes(chunk: &[u8]) -> impl Iterator<Item = usize> + '_ {
    let start = 16 + usize::from(u16_at(chunk, 24));
    let size = usize::from(u16_at(chunk, 26));
    let count = usize::from(u16_at(chunk, 28));
    (0..count).map(move |index| start + index * size)
}

fn set_string(chunk: &mut [u8], attribute: usize, index: u32) {
    chunk[attribute + 8..attribute + 12].copy_from_slice(&index.to_le_bytes());
    chunk[attribute + 15] = TYPE_STRING;
    chunk[attribute + 16..attribute + 20].copy_from_slice(&index.to_le_bytes());
}

fn set_int(chunk: &mut [u8], attribute: usize, value: u32) {
    chunk[attribute + 8..attribute + 12].copy_from_slice(&NO_STRING.to_le_bytes());
    chunk[attribute + 15] = TYPE_INT_DEC;
    chunk[attribute + 16..attribute + 20].copy_from_slice(&value.to_le_bytes());
}

/// The manifest with the app's package, label and version; permissions and
/// provider authorities named after q0player's package renamed with it; and,
/// if asked, q0player's "Open with" intent filters left out.
fn patch_manifest(bytes: &[u8], plan: &ApkPlan) -> Result<Vec<u8>, String> {
    let mut xml = Axml::parse(bytes)?;
    let app = &plan.manifest.app;
    let old_package = xml
        .chunks
        .iter()
        .filter(|chunk| u16_at(chunk, 0) == RES_XML_START_ELEMENT)
        .find(|chunk| xml.text(element_name(chunk)) == "manifest")
        .and_then(|chunk| {
            attributes(chunk)
                .find(|&attribute| xml.text(u32_at(chunk, attribute + 4)) == "package")
                .map(|attribute| xml.text(u32_at(chunk, attribute + 8)).to_string())
        })
        .ok_or("the APK template's manifest has no package")?;
    let new_package = app.id.clone();
    let package_index = xml.intern(&new_package);
    let label_index = xml.intern(&app.name);
    let version_index = xml.intern(&app.version);
    let renamed = |value: &str| -> Option<String> {
        value
            .strip_prefix(&old_package)
            .filter(|rest| rest.is_empty() || rest.starts_with('.'))
            .map(|rest| format!("{new_package}{rest}"))
    };

    let mut chunks = std::mem::take(&mut xml.chunks);
    for chunk in &mut chunks {
        if u16_at(chunk, 0) != RES_XML_START_ELEMENT {
            continue;
        }
        let element = xml.text(element_name(chunk)).to_string();
        let positions: Vec<usize> = attributes(chunk).collect();
        for attribute in positions {
            let name = xml.text(u32_at(chunk, attribute + 4)).to_string();
            let value = u32_at(chunk, attribute + 8);
            match (element.as_str(), name.as_str()) {
                ("manifest", "package") => set_string(chunk, attribute, package_index),
                ("manifest", "versionName") => set_string(chunk, attribute, version_index),
                ("manifest", "versionCode") => set_int(chunk, attribute, app.version_code()),
                ("application" | "activity", "label") => set_string(chunk, attribute, label_index),
                ("provider", "authorities") | ("permission" | "uses-permission", "name")
                    if value != NO_STRING =>
                {
                    let current = xml.text(value).to_string();
                    let new = current
                        .split(';')
                        .map(|part| renamed(part).unwrap_or_else(|| part.to_string()))
                        .collect::<Vec<_>>()
                        .join(";");
                    if new != current {
                        let index = xml.intern(&new);
                        set_string(chunk, attribute, index);
                    }
                }
                _ => {}
            }
        }
    }
    if plan.drop_open_with {
        chunks = without_view_filters(chunks, &xml);
    }
    xml.chunks = chunks;
    Ok(xml.write())
}

/// Every `<intent-filter>` that answers `android.intent.action.VIEW` left out.
fn without_view_filters(chunks: Vec<Vec<u8>>, xml: &Axml) -> Vec<Vec<u8>> {
    let mut out = Vec::with_capacity(chunks.len());
    let mut held: Option<(Vec<Vec<u8>>, usize, bool)> = None;
    for chunk in chunks {
        let kind = u16_at(&chunk, 0);
        let name = (kind == RES_XML_START_ELEMENT || kind == RES_XML_END_ELEMENT)
            .then(|| xml.text(element_name(&chunk)).to_string());
        if let Some((filter, depth, view)) = held.as_mut() {
            if kind == RES_XML_START_ELEMENT {
                *depth += 1;
                if name.as_deref() == Some("action") {
                    *view |= attributes(&chunk).any(|attribute| {
                        xml.text(u32_at(&chunk, attribute + 4)) == "name"
                            && xml.text(u32_at(&chunk, attribute + 8))
                                == "android.intent.action.VIEW"
                    });
                }
            } else if kind == RES_XML_END_ELEMENT {
                *depth -= 1;
            }
            filter.push(chunk);
            if *depth == 0 {
                let (filter, _, view) = held.take().expect("held filter");
                if !view {
                    out.extend(filter);
                }
            }
            continue;
        }
        if kind == RES_XML_START_ELEMENT && name.as_deref() == Some("intent-filter") {
            held = Some((vec![chunk], 1, false));
            continue;
        }
        out.push(chunk);
    }
    if let Some((filter, _, _)) = held {
        out.extend(filter);
    }
    out
}

// ---------------------------------------------------------------- signing

/// The key and certificate apps built on this computer are signed with.
pub struct Signer {
    key: ring::signature::EcdsaKeyPair,
    certificate: Vec<u8>,
    public_key: Vec<u8>,
    pub folder: PathBuf,
}

impl Signer {
    /// The computer's signing key, made on first use. Android installs an
    /// update only when it is signed by the same key, so the folder is
    /// worth backing up.
    pub fn load_or_create() -> Result<Self, String> {
        use ring::signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_ASN1_SIGNING};
        let folder = dirs::config_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("q0build")
            .join("android-signing");
        let key_path = folder.join("key.pk8");
        let certificate_path = folder.join("certificate.der");
        let rng = ring::rand::SystemRandom::new();
        if !key_path.is_file() {
            std::fs::create_dir_all(&folder).map_err(|error| error.to_string())?;
            let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng)
                .map_err(|_| "could not make a signing key")?;
            std::fs::write(&key_path, pkcs8.as_ref()).map_err(|error| error.to_string())?;
            std::fs::remove_file(&certificate_path).ok();
        }
        let pkcs8 = std::fs::read(&key_path).map_err(|error| error.to_string())?;
        let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &pkcs8, &rng)
            .map_err(|_| format!("{} is not a usable signing key", key_path.display()))?;
        let public_key = subject_public_key_info(key.public_key().as_ref());
        let certificate = match std::fs::read(&certificate_path) {
            Ok(certificate) => certificate,
            Err(_) => {
                let certificate = self_signed_certificate(&key, &public_key)?;
                std::fs::write(&certificate_path, &certificate)
                    .map_err(|error| error.to_string())?;
                certificate
            }
        };
        Ok(Self {
            key,
            certificate,
            public_key,
            folder,
        })
    }

    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, String> {
        let rng = ring::rand::SystemRandom::new();
        self.key
            .sign(&rng, message)
            .map(|signature| signature.as_ref().to_vec())
            .map_err(|_| "signing failed".to_string())
    }
}

fn der(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    let length = content.len();
    if length < 0x80 {
        out.push(length as u8);
    } else {
        let bytes: Vec<u8> = length
            .to_be_bytes()
            .into_iter()
            .skip_while(|byte| *byte == 0)
            .collect();
        out.push(0x80 | bytes.len() as u8);
        out.extend(bytes);
    }
    out.extend_from_slice(content);
    out
}

fn seq(parts: &[Vec<u8>]) -> Vec<u8> {
    der(0x30, &parts.concat())
}

const OID_EC_PUBLIC_KEY: &[u8] = &[0x06, 0x07, 0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x02, 0x01];
const OID_P256: &[u8] = &[0x06, 0x08, 0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x03, 0x01, 0x07];
const OID_ECDSA_SHA256: &[u8] = &[0x06, 0x08, 0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x04, 0x03, 0x02];
const OID_COMMON_NAME: &[u8] = &[0x06, 0x03, 0x55, 0x04, 0x03];

fn subject_public_key_info(point: &[u8]) -> Vec<u8> {
    let mut bits = vec![0];
    bits.extend_from_slice(point);
    seq(&[
        seq(&[OID_EC_PUBLIC_KEY.to_vec(), OID_P256.to_vec()]),
        der(0x03, &bits),
    ])
}

fn self_signed_certificate(
    key: &ring::signature::EcdsaKeyPair,
    public_key: &[u8],
) -> Result<Vec<u8>, String> {
    let name = seq(&[der(
        0x31,
        &seq(&[OID_COMMON_NAME.to_vec(), der(0x0C, b"Q0Build app signer")]),
    )]);
    let algorithm = seq(&[OID_ECDSA_SHA256.to_vec()]);
    let mut serial = [0_u8; 9];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut serial[1..])
        .map_err(|_| "no randomness")?;
    serial[1] &= 0x7F;
    let tbs = seq(&[
        der(0xA0, &der(0x02, &[2])),
        der(0x02, &serial[1..]),
        algorithm.clone(),
        name.clone(),
        seq(&[der(0x17, b"250101000000Z"), der(0x18, b"20750101000000Z")]),
        name,
        public_key.to_vec(),
    ]);
    let rng = ring::rand::SystemRandom::new();
    let signature = key
        .sign(&rng, &tbs)
        .map_err(|_| "could not sign the certificate")?;
    let mut bits = vec![0];
    bits.extend_from_slice(signature.as_ref());
    Ok(seq(&[tbs, algorithm, der(0x03, &bits)]))
}

fn prefixed(bytes: &[u8]) -> Vec<u8> {
    let mut out = (bytes.len() as u32).to_le_bytes().to_vec();
    out.extend_from_slice(bytes);
    out
}

const SIGNATURE_ECDSA_SHA256: u32 = 0x0201;
const V2_BLOCK_ID: u32 = 0x7109_871A;

/// Where the zip's end-of-central-directory record starts.
fn end_of_central_directory(apk: &[u8]) -> Option<usize> {
    let lowest = apk.len().saturating_sub(22 + 0xFFFF);
    (lowest..=apk.len().checked_sub(22)?).rev().find(|&at| {
        u32_at(apk, at) == 0x0605_4B50 && at + 22 + usize::from(u16_at(apk, at + 20)) == apk.len()
    })
}

/// SHA-256 over 1 MiB chunks of each section, the way APK v2 digests.
fn content_digest(sections: &[&[u8]]) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    let mut chunk_digests = Vec::new();
    let mut count: u32 = 0;
    for section in sections {
        for chunk in section.chunks(1 << 20) {
            let mut hasher = Sha256::new();
            hasher.update([0xA5]);
            hasher.update((chunk.len() as u32).to_le_bytes());
            hasher.update(chunk);
            chunk_digests.extend_from_slice(&hasher.finalize());
            count += 1;
        }
    }
    let mut hasher = Sha256::new();
    hasher.update([0x5A]);
    hasher.update(count.to_le_bytes());
    hasher.update(&chunk_digests);
    hasher.finalize().to_vec()
}

pub fn sign_v2(apk: Vec<u8>, signer: &Signer) -> Result<Vec<u8>, String> {
    let eocd = end_of_central_directory(&apk).ok_or("the rebuilt APK has no zip directory")?;
    let directory_size = u32_at(&apk, eocd + 12) as usize;
    let directory_offset = u32_at(&apk, eocd + 16) as usize;
    if directory_offset + directory_size != eocd {
        return Err("the rebuilt APK's zip directory is not where it should be".into());
    }
    let entries = &apk[..directory_offset];
    let directory = &apk[directory_offset..eocd];
    let end = &apk[eocd..];
    let digest = content_digest(&[entries, directory, end]);

    let mut digest_record = SIGNATURE_ECDSA_SHA256.to_le_bytes().to_vec();
    digest_record.extend(prefixed(&digest));
    let signed_data = [
        prefixed(&prefixed(&digest_record)),
        prefixed(&prefixed(&signer.certificate)),
        prefixed(&[]),
    ]
    .concat();
    let mut signature_record = SIGNATURE_ECDSA_SHA256.to_le_bytes().to_vec();
    signature_record.extend(prefixed(&signer.sign(&signed_data)?));
    let signer_record = [
        prefixed(&signed_data),
        prefixed(&prefixed(&signature_record)),
        prefixed(&signer.public_key),
    ]
    .concat();
    let v2_value = prefixed(&prefixed(&signer_record));

    let mut pair = ((v2_value.len() + 4) as u64).to_le_bytes().to_vec();
    pair.extend_from_slice(&V2_BLOCK_ID.to_le_bytes());
    pair.extend_from_slice(&v2_value);
    let block_size = (pair.len() + 8 + 16) as u64;
    let mut block = block_size.to_le_bytes().to_vec();
    block.extend_from_slice(&pair);
    block.extend_from_slice(&block_size.to_le_bytes());
    block.extend_from_slice(b"APK Sig Block 42");

    let mut out = Vec::with_capacity(apk.len() + block.len());
    out.extend_from_slice(entries);
    out.extend_from_slice(&block);
    out.extend_from_slice(directory);
    let mut end = end.to_vec();
    end[16..20].copy_from_slice(&((directory_offset + block.len()) as u32).to_le_bytes());
    out.extend_from_slice(&end);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny binary manifest in aapt2's layout: UTF-16 pool, then
    /// <manifest package versionCode versionName><application label>
    /// <intent-filter><action name=VIEW/></intent-filter></application>.
    fn sample_manifest() -> Vec<u8> {
        let strings = [
            "versionCode",
            "versionName",
            "label",
            "name",
            "package",
            "manifest",
            "application",
            "intent-filter",
            "action",
            "android.intent.action.VIEW",
            "com.q0id.q0player",
            "1.0",
            "q0player",
            "com.q0id.q0player.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION",
            "permission",
        ];
        let xml = Axml {
            strings: strings.iter().map(|s| s.to_string()).collect(),
            utf8: false,
            chunks: Vec::new(),
        };
        let index = |text: &str| strings.iter().position(|s| *s == text).unwrap() as u32;
        let element = |name: &str, attrs: &[(&str, u8, u32)]| -> Vec<u8> {
            let mut chunk = Vec::new();
            chunk.extend_from_slice(&RES_XML_START_ELEMENT.to_le_bytes());
            chunk.extend_from_slice(&16_u16.to_le_bytes());
            chunk.extend_from_slice(&0_u32.to_le_bytes());
            chunk.extend_from_slice(&1_u32.to_le_bytes());
            chunk.extend_from_slice(&NO_STRING.to_le_bytes());
            chunk.extend_from_slice(&NO_STRING.to_le_bytes());
            chunk.extend_from_slice(&index(name).to_le_bytes());
            for value in [20_u16, 20, attrs.len() as u16, 0, 0, 0] {
                chunk.extend_from_slice(&value.to_le_bytes());
            }
            for (attr, kind, data) in attrs {
                chunk.extend_from_slice(&NO_STRING.to_le_bytes());
                chunk.extend_from_slice(&index(attr).to_le_bytes());
                let raw = if *kind == TYPE_STRING {
                    *data
                } else {
                    NO_STRING
                };
                chunk.extend_from_slice(&raw.to_le_bytes());
                chunk.extend_from_slice(&8_u16.to_le_bytes());
                chunk.push(0);
                chunk.push(*kind);
                chunk.extend_from_slice(&data.to_le_bytes());
            }
            let size = chunk.len() as u32;
            chunk[4..8].copy_from_slice(&size.to_le_bytes());
            chunk
        };
        let end = |name: &str| -> Vec<u8> {
            let mut chunk = Vec::new();
            chunk.extend_from_slice(&RES_XML_END_ELEMENT.to_le_bytes());
            chunk.extend_from_slice(&16_u16.to_le_bytes());
            chunk.extend_from_slice(&24_u32.to_le_bytes());
            chunk.extend_from_slice(&1_u32.to_le_bytes());
            chunk.extend_from_slice(&NO_STRING.to_le_bytes());
            chunk.extend_from_slice(&NO_STRING.to_le_bytes());
            chunk.extend_from_slice(&index(name).to_le_bytes());
            chunk
        };
        let mut xml = xml;
        xml.chunks = vec![
            element(
                "manifest",
                &[
                    ("package", TYPE_STRING, index("com.q0id.q0player")),
                    ("versionCode", TYPE_INT_DEC, 1),
                    ("versionName", TYPE_STRING, index("1.0")),
                ],
            ),
            element(
                "permission",
                &[(
                    "name",
                    TYPE_STRING,
                    index("com.q0id.q0player.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION"),
                )],
            ),
            end("permission"),
            element("application", &[("label", TYPE_STRING, index("q0player"))]),
            element("intent-filter", &[]),
            element(
                "action",
                &[("name", TYPE_STRING, index("android.intent.action.VIEW"))],
            ),
            end("action"),
            end("intent-filter"),
            end("application"),
            end("manifest"),
        ];
        xml.write()
    }

    fn plan_manifest() -> q0qir::Manifest {
        let mut manifest = q0qir::Manifest::default();
        manifest.app.id = "ru.shadowmol.grunt".into();
        manifest.app.name = "Грант".into();
        manifest.app.version = "2.1.0".into();
        manifest
    }

    #[test]
    fn the_manifest_takes_the_apps_name_package_and_version() {
        let manifest = plan_manifest();
        let icon = crate::icons::placeholder("x", [0, 0, 0]);
        let plan = ApkPlan {
            manifest: &manifest,
            bundle: b"",
            icon: &icon,
            drop_open_with: true,
        };
        let patched = Axml::parse(&patch_manifest(&sample_manifest(), &plan).unwrap()).unwrap();
        let attr = |element: &str, name: &str| -> (u8, u32) {
            let chunk = patched
                .chunks
                .iter()
                .find(|c| {
                    u16_at(c, 0) == RES_XML_START_ELEMENT
                        && patched.text(element_name(c)) == element
                })
                .unwrap();
            let at = attributes(chunk)
                .find(|&a| patched.text(u32_at(chunk, a + 4)) == name)
                .unwrap();
            (chunk[at + 15], u32_at(chunk, at + 16))
        };
        assert_eq!(
            patched.text(attr("manifest", "package").1),
            "ru.shadowmol.grunt"
        );
        assert_eq!(patched.text(attr("manifest", "versionName").1), "2.1.0");
        assert_eq!(attr("manifest", "versionCode"), (TYPE_INT_DEC, 20100));
        assert_eq!(patched.text(attr("application", "label").1), "Грант");
        assert_eq!(
            patched.text(attr("permission", "name").1),
            "ru.shadowmol.grunt.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION"
        );
        assert!(
            !patched
                .chunks
                .iter()
                .any(|c| u16_at(c, 0) == RES_XML_START_ELEMENT
                    && patched.text(element_name(c)) == "intent-filter"),
            "the VIEW filter is gone"
        );
        // The first strings, which the resource map names, keep their places.
        assert_eq!(
            patched.strings[..5],
            ["versionCode", "versionName", "label", "name", "package"]
        );
    }

    #[test]
    fn utf8_pools_read_back() {
        let mut xml = Axml::parse(&sample_manifest()).unwrap();
        xml.utf8 = true;
        xml.intern("é and a long string past one hundred and twenty-seven bytes ...................................................................");
        let back = Axml::parse(&xml.write()).unwrap();
        assert_eq!(back.strings, xml.strings);
    }

    #[test]
    fn the_v2_block_sits_before_the_directory_and_the_end_points_at_it() {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file("a.txt", zip::write::FileOptions::default())
            .unwrap();
        writer.write_all(b"hello").unwrap();
        let apk = writer.finish().unwrap().into_inner();
        let old_eocd = end_of_central_directory(&apk).unwrap();
        let old_offset = u32_at(&apk, old_eocd + 16) as usize;
        let folder = std::env::temp_dir().join(format!("q0build-sign-{}", std::process::id()));
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = ring::signature::EcdsaKeyPair::generate_pkcs8(
            &ring::signature::ECDSA_P256_SHA256_ASN1_SIGNING,
            &rng,
        )
        .unwrap();
        let key = ring::signature::EcdsaKeyPair::from_pkcs8(
            &ring::signature::ECDSA_P256_SHA256_ASN1_SIGNING,
            pkcs8.as_ref(),
            &rng,
        )
        .unwrap();
        use ring::signature::KeyPair;
        let public_key = subject_public_key_info(key.public_key().as_ref());
        let certificate = self_signed_certificate(&key, &public_key).unwrap();
        let signer = Signer {
            key,
            certificate,
            public_key,
            folder,
        };
        let signed = sign_v2(apk.clone(), &signer).unwrap();
        let eocd = end_of_central_directory(&signed).unwrap();
        let offset = u32_at(&signed, eocd + 16) as usize;
        assert_eq!(&signed[offset - 16..offset], b"APK Sig Block 42");
        assert_eq!(
            &signed[offset..offset + 4],
            &0x0201_4B50_u32.to_le_bytes(),
            "directory follows the block"
        );
        assert_eq!(&signed[..old_offset], &apk[..old_offset]);
        assert!(zip::ZipArchive::new(Cursor::new(signed)).is_ok());
    }
}
