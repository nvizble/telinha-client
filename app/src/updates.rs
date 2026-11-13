use anyhow::{Context, ensure};
use parking_lot::Mutex;
#[cfg(target_os = "windows")]
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc, time::Duration};
use telinha_shared::updates;
#[cfg(any(target_os = "windows", test))]
use {std::path::Path, telinha_shared::updates::Release};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prepared {
    pub version: String,
    pub executable: PathBuf,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum State {
    #[default]
    Idle,
    Downloading {
        version: String,
        percent: u8,
    },
    Ready(Prepared),
}
pub type Status = Arc<Mutex<State>>;

pub async fn run(status: Status) {
    if !cfg!(target_os = "windows") {
        return;
    }
    tokio::time::sleep(Duration::from_secs(3)).await;
    loop {
        let retry = match check(&status).await {
            Ok(()) => Duration::from_secs(6 * 3600),
            Err(error) => {
                tracing::warn!("Não foi possível verificar a atualização: {error}");
                if matches!(*status.lock(), State::Downloading { .. }) {
                    *status.lock() = State::Idle;
                }
                Duration::from_secs(300)
            }
        };
        tokio::time::sleep(retry).await;
    }
}

async fn read_manifest(mut response: reqwest::Response) -> anyhow::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            bytes.len() + chunk.len() <= updates::MAX_MANIFEST,
            "Manifesto grande demais."
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn check(status: &Status) -> anyhow::Result<()> {
    use tokio::io::AsyncWriteExt;
    let client = reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(300))
        .build()?;
    let response = client.get(updates::MANIFEST_URL).send().await?;
    if response.status() == reqwest::StatusCode::NO_CONTENT {
        return Ok(());
    }
    let signed = read_manifest(response.error_for_status()?).await?;
    let release = updates::verify(&signed)?;
    if !updates::newer(&release.version, env!("CARGO_PKG_VERSION"))? {
        return Ok(());
    }
    let root = PathBuf::from(
        std::env::var_os("LOCALAPPDATA").context("Pasta local de atualizações indisponível.")?,
    )
    .join("telinha")
    .join("updates");
    let directory = root.join(&release.sha256);
    tokio::fs::create_dir_all(&directory).await?;
    let executable = directory.join("telinha.exe");
    let candidate = executable.clone();
    let expected = release.clone();
    let cached =
        tokio::task::spawn_blocking(move || updates::verify_file(&candidate, &expected).is_ok())
            .await?;
    if !cached {
        *status.lock() = State::Downloading {
            version: release.version.clone(),
            percent: 0,
        };
        let temporary = directory.join(format!(
            "download-{}-{}.part",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        // Declare the guard first so Windows closes the file before cleanup on error.
        let cleanup;
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .await?;
        cleanup = RemoveFile(temporary.clone());
        let mut response = client.get(&release.url).send().await?.error_for_status()?;
        if let Some(length) = response.content_length() {
            ensure!(length == release.size, "Tamanho inesperado na atualização.");
        }
        let mut received = 0;
        while let Some(chunk) = response.chunk().await? {
            received += chunk.len() as u64;
            ensure!(
                received <= release.size,
                "Download excedeu o tamanho assinado."
            );
            file.write_all(&chunk).await?;
            *status.lock() = State::Downloading {
                version: release.version.clone(),
                percent: (received * 100 / release.size) as u8,
            };
        }
        file.sync_all().await?;
        drop(file);
        ensure!(received == release.size, "Download incompleto.");
        let candidate = temporary.clone();
        let expected = release.clone();
        tokio::task::spawn_blocking(move || updates::verify_file(&candidate, &expected)).await??;
        // Another app instance may have finished the same download meanwhile.
        let candidate = executable.clone();
        let expected = release.clone();
        let cached = tokio::task::spawn_blocking(move || {
            updates::verify_file(&candidate, &expected).is_ok()
        })
        .await?;
        if !cached {
            // Only replace a corrupt candidate inside our cache, never the app.
            if executable.exists() {
                tokio::fs::remove_file(&executable).await?;
            }
            tokio::fs::rename(&temporary, &executable).await?;
        }
        drop(cleanup);
    }
    let manifest = directory.join("manifest.json");
    let temporary = directory.join(format!("manifest-{}.part", std::process::id()));
    let cleanup = RemoveFile(temporary.clone());
    tokio::fs::write(&temporary, signed).await?;
    tokio::fs::rename(&temporary, &manifest).await?;
    drop(cleanup);
    *status.lock() = State::Ready(Prepared {
        version: release.version,
        executable,
    });
    Ok(())
}

struct RemoveFile(PathBuf);
impl Drop for RemoveFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(target_os = "windows")]
#[derive(Serialize, Deserialize)]
struct Install {
    parent_pid: u32,
    target: PathBuf,
}

pub fn launch(prepared: &Prepared) -> anyhow::Result<()> {
    #[cfg(not(target_os = "windows"))]
    {
        let _ = prepared;
        anyhow::bail!("A atualização automática está disponível no Windows.");
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        let target = std::env::current_exe()?.canonicalize()?;
        let directory = prepared
            .executable
            .parent()
            .context("Atualização sem pasta.")?;
        let manifest = directory.join("manifest.json");
        ensure!(
            std::fs::metadata(&manifest)?.len() <= updates::MAX_MANIFEST as u64,
            "Manifesto inválido."
        );
        let release = updates::verify(&std::fs::read(manifest)?)?;
        ensure!(
            release.version == prepared.version
                && updates::newer(&release.version, env!("CARGO_PKG_VERSION"))?,
            "Versão de atualização inválida."
        );
        // Recheck immediately before executing the downloaded helper.
        updates::verify_file(&prepared.executable, &release)?;
        let plan = directory.join(format!("install-{}.json", std::process::id()));
        std::fs::write(
            &plan,
            serde_json::to_vec(&Install {
                parent_pid: std::process::id(),
                target,
            })?,
        )?;
        std::process::Command::new(&prepared.executable)
            .arg("--apply-update")
            .arg(&plan)
            .creation_flags(0x08000000)
            .spawn()
            .context("Não deu pra abrir o atualizador. Tente novamente.")?;
        Ok(())
    }
}

#[cfg(any(target_os = "windows", test))]
fn replace(candidate: &Path, target: &Path, release: &Release) -> anyhow::Result<PathBuf> {
    updates::verify_file(candidate, release)?;
    ensure!(
        target.is_absolute() && target.is_file() && candidate != target,
        "Destino de atualização inválido."
    );
    let directory = target.parent().context("Destino sem pasta.")?;
    let temporary = directory.join(format!(".telinha-{}-new.exe", std::process::id()));
    let backup = directory.join(format!(".telinha-{}-previous.exe", std::process::id()));
    ensure!(!backup.exists(), "Já existe uma atualização em andamento.");
    let cleanup;
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .context("Sem permissão para atualizar nesta pasta. Baixe a nova versão pelo site.")?;
    cleanup = RemoveFile(temporary.clone());
    std::io::copy(&mut std::fs::File::open(candidate)?, &mut output)?;
    output.sync_all()?;
    drop(output);
    updates::verify_file(&temporary, release)?;
    rename_retry(target, &backup)?;
    if let Err(error) = rename_retry(&temporary, target) {
        // Preserve the backup even if a third party prevents restoring the name.
        let _ = rename_retry(&backup, target);
        return Err(error);
    }
    drop(cleanup);
    Ok(backup)
}

#[cfg(any(target_os = "windows", test))]
fn rename_retry(from: &Path, to: &Path) -> anyhow::Result<()> {
    for attempt in 0..50 {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(error) if attempt == 49 => {
                return Err(error)
                    .context("O executável está em uso ou a pasta não permite atualização.");
            }
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
    unreachable!()
}

pub fn worker_main() -> anyhow::Result<()> {
    #[cfg(not(target_os = "windows"))]
    anyhow::bail!("Atualizador disponível apenas no Windows.");
    #[cfg(target_os = "windows")]
    {
        use std::process::Command;
        use windows::Win32::{
            Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, WAIT_OBJECT_0},
            System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
        };
        let candidate = std::env::current_exe()?.canonicalize()?;
        let directory = candidate.parent().context("Atualização sem pasta.")?;
        let plan_path = PathBuf::from(
            std::env::args_os()
                .nth(2)
                .context("Atualização sem instruções.")?,
        )
        .canonicalize()?;
        ensure!(
            plan_path.parent() == Some(directory),
            "Instruções fora da pasta da atualização."
        );
        ensure!(
            std::fs::metadata(&plan_path)?.len() <= 8192,
            "Instruções inválidas."
        );
        let plan: Install = serde_json::from_slice(&std::fs::read(&plan_path)?)?;
        let result = (|| -> anyhow::Result<()> {
            let manifest = directory.join("manifest.json");
            ensure!(
                std::fs::metadata(&manifest)?.len() <= updates::MAX_MANIFEST as u64,
                "Manifesto inválido."
            );
            let release = updates::verify(&std::fs::read(manifest)?)?;
            ensure!(
                release.version == env!("CARGO_PKG_VERSION"),
                "O executável não corresponde à versão assinada."
            );
            updates::verify_file(&candidate, &release)?;
            match unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, plan.parent_pid) } {
                Ok(handle) => {
                    let status = unsafe { WaitForSingleObject(handle, 30_000) };
                    let _ = unsafe { CloseHandle(handle) };
                    ensure!(
                        status == WAIT_OBJECT_0,
                        "O Telinha não encerrou a tempo de atualizar."
                    );
                }
                Err(error)
                    if error.code()
                        == windows::core::HRESULT::from_win32(ERROR_INVALID_PARAMETER.0) => {}
                Err(error) => return Err(error.into()),
            }
            let backup = replace(&candidate, &plan.target, &release)?;
            match Command::new(&plan.target).spawn() {
                Ok(_) => {
                    let _ = std::fs::remove_file(&backup);
                }
                Err(error) => {
                    let _ = std::fs::remove_file(&plan.target);
                    let _ = rename_retry(&backup, &plan.target);
                    return Err(error).context("Não foi possível reabrir o app atualizado.");
                }
            }
            Ok(())
        })();
        let _ = std::fs::remove_file(plan_path);
        if result.is_err() {
            // Reopen the intact old app, without rerunning the installer.
            let _ = Command::new(&plan.target)
                .env(
                    "TELINHA_UPDATE_ERROR",
                    "Não foi possível concluir a atualização. Tente novamente ou baixe pelo site.",
                )
                .spawn();
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(directory: &Path) -> (PathBuf, PathBuf, Release) {
        let target = directory.join("telinha.exe");
        let candidate = directory.join("new.exe");
        let mut bytes = vec![0; 128];
        bytes[..2].copy_from_slice(b"MZ");
        bytes[60..64].copy_from_slice(&64u32.to_le_bytes());
        bytes[64..70].copy_from_slice(b"PE\0\0\x64\x86");
        std::fs::write(&candidate, &bytes).unwrap();
        std::fs::write(&target, b"previous app").unwrap();
        let (size, sha256) = updates::digest(&candidate).unwrap();
        let release = Release {
            schema: 1,
            version: "0.6.0".into(),
            platform: "windows-x86_64".into(),
            url: updates::artifact_url(&sha256),
            sha256,
            size,
        };
        (candidate, target, release)
    }
    #[test]
    fn install_preserves_old_executable_and_rejects_corrupt_downloads() {
        let directory = tempfile::tempdir().unwrap();
        let (candidate, target, release) = fixture(directory.path());
        let backup = replace(&candidate, &target, &release).unwrap();
        assert_eq!(std::fs::read(&backup).unwrap(), b"previous app");
        updates::verify_file(&target, &release).unwrap();
        std::fs::remove_file(backup).unwrap();
        std::fs::write(&candidate, b"corrupt").unwrap();
        assert!(replace(&candidate, &target, &release).is_err());
        updates::verify_file(&target, &release).unwrap();
    }
    #[cfg(target_os = "windows")]
    #[test]
    fn install_waits_for_windows_file_locks_without_deleting_the_app() {
        use std::os::windows::fs::OpenOptionsExt;
        let directory = tempfile::tempdir().unwrap();
        let (candidate, target, release) = fixture(directory.path());
        let locked = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&target)
            .unwrap();
        let thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            drop(locked);
        });
        let backup = replace(&candidate, &target, &release).unwrap();
        thread.join().unwrap();
        assert_eq!(std::fs::read(backup).unwrap(), b"previous app");
        updates::verify_file(&target, &release).unwrap();
    }
}
