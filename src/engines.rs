//! Downloadable UCI engines: catalog, install directory, background downloads.
//! Engines go to ~/.local/share/chess-rt/engines/<id>/; `path.txt` there names the executable.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub struct Entry {
    pub id: &'static str,
    pub name: &'static str,
    pub repo: &'static str,
    /// the release asset whose name contains all of these (and none of `avoid`)
    pub want: &'static [&'static str],
    pub avoid: &'static [&'static str],
    pub about: &'static str,
}

pub const CATALOG: [Entry; 6] = [
    Entry {
        id: "stockfish",
        name: "Stockfish",
        repo: "official-stockfish/Stockfish",
        want: &["linux-x86-64"],
        avoid: &[],
        about: "сильнейший движок; сила настраивается от 1320 до 3190 Elo",
        },
    Entry {
        id: "fairy",
        name: "Fairy-Stockfish",
        repo: "fairy-stockfish/Fairy-Stockfish",
        want: &["fairy-stockfish_x86-64-bmi2"],
        avoid: &["largeboard"],
        about: "лёгкий (2 МБ) родственник Stockfish; есть уровень мастерства и Elo",
    },
    Entry {
        id: "koivisto",
        name: "Koivisto",
        repo: "Luecx/Koivisto",
        want: &["linux-avx2"],
        avoid: &[],
        about: "сильный движок с нейросетью; сила — ограничением перебора",
    },
    Entry {
        id: "reckless",
        name: "Reckless",
        repo: "codedeliveryservice/Reckless",
        want: &["linux-avx2"],
        avoid: &[],
        about: "один из сильнейших (Rust); сила — ограничением перебора",
    },
    Entry {
        id: "viridithas",
        name: "Viridithas",
        repo: "cosmobobak/viridithas",
        want: &["linux-x86-64-v3"],
        avoid: &[],
        about: "очень сильный (Rust); сила — ограничением перебора",
    },
    Entry {
        id: "princhess",
        name: "Princhess",
        repo: "princesslana/princhess",
        want: &["princhess"],
        avoid: &[".", "-"],
        about: "играет методом Монте-Карло (MCTS), необычный стиль",
    },
];

pub fn entry(id: &str) -> Option<&'static Entry> {
    CATALOG.iter().find(|e| e.id == id)
}

pub fn root() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share"));
    base.join("chess-rt/engines")
}

pub fn installed_path(id: &str) -> Option<PathBuf> {
    let dir = root().join(id);
    let p = std::fs::read_to_string(dir.join("path.txt")).ok()?;
    let p = dir.join(p.trim());
    p.exists().then_some(p)
}

pub fn installed_name(id: &str) -> Option<String> {
    std::fs::read_to_string(root().join(id).join("name.txt")).ok().map(|s| s.trim().to_string())
}

/// (kind, elo min, elo max) recorded when the engine was installed.
pub fn installed_strength(id: &str) -> (String, i64, i64) {
    let s = std::fs::read_to_string(root().join(id).join("strength.txt")).unwrap_or_default();
    let t: Vec<&str> = s.split_whitespace().collect();
    (
        t.first().unwrap_or(&"nodes").to_string(),
        t.get(1).and_then(|v| v.parse().ok()).unwrap_or(0),
        t.get(2).and_then(|v| v.parse().ok()).unwrap_or(0),
    )
}

pub fn installed_ids() -> Vec<&'static str> {
    CATALOG.iter().filter(|e| installed_path(e.id).is_some()).map(|e| e.id).collect()
}

pub fn delete(id: &str) {
    let _ = std::fs::remove_dir_all(root().join(id));
}

#[derive(Clone, Debug)]
pub enum Dl {
    Looking,
    Downloading { got: u64, total: u64 },
    Unpacking,
    Checking,
    Done,
    Failed(String),
}

pub type DlState = Arc<Mutex<(Dl, bool)>>; // (state, cancel requested)

fn set(st: &DlState, d: Dl) {
    st.lock().unwrap().0 = d;
}
fn cancelled(st: &DlState) -> bool {
    st.lock().unwrap().1
}

/// Start downloading `id` in the background; progress goes into the returned state.
pub fn download(id: &'static str) -> DlState {
    let st: DlState = Arc::new(Mutex::new((Dl::Looking, false)));
    let s2 = st.clone();
    std::thread::spawn(move || {
        if let Err(e) = run_download(id, &s2) {
            let msg = if cancelled(&s2) { "отменено".to_string() } else { e };
            let _ = std::fs::remove_dir_all(root().join(id).join("tmp"));
            set(&s2, Dl::Failed(msg));
        }
    });
    st
}

fn run_download(id: &str, st: &DlState) -> Result<(), String> {
    let e = entry(id).ok_or("нет в каталоге")?;
    // 1. find the asset in the latest release
    let api = format!("https://api.github.com/repos/{}/releases/latest", e.repo);
    let out = Command::new("curl")
        .args(["-sL", "-m", "25", "-H", "Accept: application/vnd.github+json", "-A", "chess-rt", &api])
        .output()
        .map_err(|e| format!("нет curl: {e}"))?;
    let json: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|_| "GitHub не ответил (нет сети?)".to_string())?;
    let assets = json["assets"].as_array().ok_or("в релизе нет файлов")?;
    let asset = assets
        .iter()
        .find(|a| {
            let n = a["name"].as_str().unwrap_or("");
            e.want.iter().all(|w| n.contains(w)) && !e.avoid.iter().any(|w| n.contains(w))
        })
        .ok_or("не найдена сборка для Linux x86-64")?;
    let name = asset["name"].as_str().unwrap_or("engine").to_string();
    let url = asset["browser_download_url"].as_str().ok_or("нет ссылки")?.to_string();
    let total = asset["size"].as_u64().unwrap_or(0);

    // 2. download with curl, watching the file grow
    let dir = root().join(id);
    let tmp = dir.join("tmp");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let file = tmp.join(&name);
    let mut child = Command::new("curl")
        .args(["-sL", "--fail", "-A", "chess-rt", "-o"])
        .arg(&file)
        .arg(&url)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("нет curl: {e}"))?;
    loop {
        if cancelled(st) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("отменено".into());
        }
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            if !status.success() {
                return Err("загрузка не удалась".into());
            }
            break;
        }
        let got = std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
        set(st, Dl::Downloading { got, total });
        std::thread::sleep(Duration::from_millis(150));
    }

    // 3. unpack and find the executable
    set(st, Dl::Unpacking);
    let unpacked = tmp.join("x");
    std::fs::create_dir_all(&unpacked).map_err(|e| e.to_string())?;
    let lower = name.to_lowercase();
    let exe: PathBuf = if lower.ends_with(".tar") || lower.ends_with(".tar.gz") || lower.ends_with(".tgz") || lower.ends_with(".tar.xz") {
        run(Command::new("tar").arg("-xf").arg(&file).arg("-C").arg(&unpacked))?;
        find_exe(&unpacked, id).ok_or("в архиве не найден исполняемый файл")?
    } else if lower.ends_with(".zip") {
        run(Command::new("unzip").arg("-q").arg(&file).arg("-d").arg(&unpacked))?;
        find_exe(&unpacked, id).ok_or("в архиве не найден исполняемый файл")?
    } else {
        let dst = unpacked.join(id);
        std::fs::rename(&file, &dst).map_err(|e| e.to_string())?;
        dst
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755));
    }

    // 4. make sure it speaks UCI before we keep it
    set(st, Dl::Checking);
    let eng = crate::uci::Engine::start(&exe)?;
    let eng_name = if eng.name.is_empty() { e.name.to_string() } else { eng.name.clone() };
    let (lo, hi) = eng.elo_range().unwrap_or((0, 0));
    let strength = format!("{} {} {}", eng.strength_kind(), lo, hi);
    drop(eng);

    // 5. move into place
    let final_dir = dir.join("bin");
    let _ = std::fs::remove_dir_all(&final_dir);
    std::fs::rename(&unpacked, &final_dir).map_err(|e| e.to_string())?;
    let rel = exe.strip_prefix(&unpacked).unwrap_or(&exe).to_path_buf();
    std::fs::write(dir.join("path.txt"), Path::new("bin").join(rel).to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("name.txt"), eng_name.as_bytes()).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("strength.txt"), strength.as_bytes()).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(&tmp);
    set(st, Dl::Done);
    Ok(())
}

fn run(c: &mut Command) -> Result<(), String> {
    let s = c.status().map_err(|e| e.to_string())?;
    if s.success() {
        Ok(())
    } else {
        Err("не удалось распаковать".into())
    }
}

/// Largest executable file whose name mentions the engine (falls back to the largest executable).
fn find_exe(dir: &Path, id: &str) -> Option<PathBuf> {
    let mut all = vec![];
    collect(dir, &mut all);
    let is_exe = |p: &Path| {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(p).map(|m| m.permissions().mode() & 0o111 != 0).unwrap_or(false)
        }
        #[cfg(not(unix))]
        {
            true
        }
    };
    let bad = |p: &Path| {
        let n = p.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
        n.ends_with(".nnue") || n.ends_with(".txt") || n.ends_with(".md") || n.ends_with(".ini") || n.ends_with(".so") || n.contains("license") || n.contains("readme")
    };
    let size = |p: &PathBuf| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    let named = all
        .iter()
        .filter(|p| !bad(p) && p.file_name().unwrap_or_default().to_string_lossy().to_lowercase().contains(id))
        .filter(|p| is_exe(p))
        .max_by_key(|p| size(p))
        .cloned();
    named.or_else(|| all.iter().filter(|p| !bad(p) && is_exe(p)).max_by_key(|p| size(p)).cloned())
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                collect(&p, out);
            } else {
                out.push(p);
            }
        }
    }
}
