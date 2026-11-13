use anyhow::{Context, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use ring::signature::{self, UnparsedPublicKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

pub const PUBLIC_KEY: &[u8; 32] = include_bytes!("../assets/update-key.bin");
pub const MANIFEST_URL: &str = "https://telinha.tech/updates/windows/latest.json";
pub const MAX_MANIFEST: usize = 16 * 1024;
pub const MAX_EXE: u64 = 200 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Release {
    pub schema: u32,
    pub version: String,
    pub platform: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SignedRelease {
    pub payload: String,
    pub signature: String,
}

pub fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub fn artifact_url(hash: &str) -> String {
    format!("https://telinha.tech/updates/windows/{hash}/telinha.exe")
}
pub fn verify(bytes: &[u8]) -> anyhow::Result<Release> {
    verify_with_key(bytes, PUBLIC_KEY)
}

fn verify_with_key(bytes: &[u8], key: &[u8]) -> anyhow::Result<Release> {
    ensure!(
        bytes.len() <= MAX_MANIFEST,
        "Manifesto de atualização grande demais."
    );
    let signed: SignedRelease = serde_json::from_slice(bytes)?;
    let payload = STANDARD.decode(signed.payload)?;
    let signature = STANDARD.decode(signed.signature)?;
    UnparsedPublicKey::new(&signature::ED25519, key)
        .verify(&payload, &signature)
        .map_err(|_| anyhow::anyhow!("A assinatura da atualização não é válida."))?;
    let release: Release = serde_json::from_slice(&payload)?;
    let version = semver::Version::parse(&release.version)?;
    ensure!(
        release.schema == 1 && release.platform == "windows-x86_64",
        "Atualização incompatível."
    );
    ensure!(
        version.pre.is_empty() && version.build.is_empty(),
        "Versão de atualização inválida."
    );
    ensure!(
        (1..=MAX_EXE).contains(&release.size) && valid_hash(&release.sha256),
        "Metadados da atualização inválidos."
    );
    // No third-party origin, redirect target, query or mutable artifact URL.
    ensure!(
        release.url == artifact_url(&release.sha256),
        "Endereço da atualização inválido."
    );
    Ok(release)
}

pub fn newer(version: &str, current: &str) -> anyhow::Result<bool> {
    Ok(semver::Version::parse(version)? > semver::Version::parse(current)?)
}

pub fn digest(path: &Path) -> anyhow::Result<(u64, String)> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    ensure!(
        (1..=MAX_EXE).contains(&size),
        "Tamanho do executável inválido."
    );
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    let mut read = 0u64;
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        read += n as u64;
        ensure!(read <= MAX_EXE, "Executável grande demais.");
        hash.update(&buffer[..n]);
    }
    ensure!(read == size, "O executável mudou durante a verificação.");
    Ok((size, format!("{:x}", hash.finalize())))
}

pub fn verify_file(path: &Path, release: &Release) -> anyhow::Result<()> {
    let (size, hash) = digest(path)?;
    ensure!(
        size == release.size && hash == release.sha256,
        "A atualização está incompleta ou foi alterada."
    );
    let mut file = File::open(path)?;
    let mut header = [0u8; 64];
    file.read_exact(&mut header)
        .context("Executável inválido.")?;
    ensure!(
        &header[..2] == b"MZ",
        "O arquivo não é um executável Windows."
    );
    let offset = u32::from_le_bytes(header[60..64].try_into()?) as u64;
    ensure!(
        offset >= 64 && offset.checked_add(6).is_some_and(|end| end <= size),
        "Cabeçalho PE inválido."
    );
    file.seek(SeekFrom::Start(offset))?;
    let mut pe = [0; 6];
    file.read_exact(&mut pe)?;
    ensure!(&pe == b"PE\0\0\x64\x86", "A atualização não é Windows x64.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::{
        rand::SystemRandom,
        signature::{Ed25519KeyPair, KeyPair},
    };

    fn fixture() -> (Ed25519KeyPair, Release) {
        let key = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
        let key = Ed25519KeyPair::from_pkcs8(key.as_ref()).unwrap();
        let hash = "a".repeat(64);
        (
            key,
            Release {
                schema: 1,
                version: "0.6.0".into(),
                platform: "windows-x86_64".into(),
                url: artifact_url(&hash),
                sha256: hash,
                size: 100,
            },
        )
    }
    fn signed(key: &Ed25519KeyPair, release: &Release) -> Vec<u8> {
        let payload = serde_json::to_vec(release).unwrap();
        serde_json::to_vec(&SignedRelease {
            signature: STANDARD.encode(key.sign(&payload).as_ref()),
            payload: STANDARD.encode(payload),
        })
        .unwrap()
    }
    #[test]
    fn signature_binds_payload_and_rejects_wrong_key_and_untrusted_urls() {
        let (key, mut release) = fixture();
        let bytes = signed(&key, &release);
        assert_eq!(
            verify_with_key(&bytes, key.public_key().as_ref()).unwrap(),
            release
        );
        assert!(verify_with_key(&bytes, fixture().0.public_key().as_ref()).is_err());
        let mut envelope: SignedRelease = serde_json::from_slice(&bytes).unwrap();
        release.version = "99.0.0".into();
        envelope.payload = STANDARD.encode(serde_json::to_vec(&release).unwrap());
        assert!(
            verify_with_key(
                &serde_json::to_vec(&envelope).unwrap(),
                key.public_key().as_ref()
            )
            .is_err()
        );
        for url in [
            "http://telinha.tech/baixar",
            "https://example.com/telinha.exe",
            "https://telinha.tech/baixar",
        ] {
            release.url = url.into();
            assert!(verify_with_key(&signed(&key, &release), key.public_key().as_ref()).is_err());
        }
        assert!(verify(&vec![b' '; MAX_MANIFEST + 1]).is_err());
    }
    #[test]
    fn versions_never_downgrade_and_compare_numerically() {
        assert!(!newer("0.5.1", "0.6.0").unwrap());
        assert!(!newer("0.6.0", "0.6.0").unwrap());
        assert!(newer("0.10.0", "0.9.0").unwrap());
        assert!(newer("invalid", "0.6.0").is_err());
    }
    #[test]
    fn binary_validation_rejects_corruption_and_wrong_architecture() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut bytes = vec![0; 128];
        bytes[..2].copy_from_slice(b"MZ");
        bytes[60..64].copy_from_slice(&64u32.to_le_bytes());
        bytes[64..70].copy_from_slice(b"PE\0\0\x64\x86");
        std::fs::write(file.path(), &bytes).unwrap();
        let (_, mut release) = fixture();
        (release.size, release.sha256) = digest(file.path()).unwrap();
        verify_file(file.path(), &release).unwrap();
        bytes[100] = 1;
        std::fs::write(file.path(), &bytes).unwrap();
        assert!(verify_file(file.path(), &release).is_err());
        bytes[68..70].copy_from_slice(&[0x4c, 1]);
        std::fs::write(file.path(), &bytes).unwrap();
        (release.size, release.sha256) = digest(file.path()).unwrap();
        assert!(verify_file(file.path(), &release).is_err());
    }
}
