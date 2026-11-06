//! Models of the built-in recognition that RustNVR can download itself
//! (Paramètres → Intelligence artificielle), checked by their SHA-256.

use sha2::{Digest as _, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::AsyncWriteExt as _;

/// A downloadable model.
#[derive(Clone, Copy, Debug)]
pub struct ModelInfo {
    /// Identifier in the API (`yolox_nano`).
    pub id: &'static str,
    /// Name shown.
    pub name: &'static str,
    /// File name in the models directory.
    pub file: &'static str,
    /// Where it is downloaded from (official publication).
    pub url: &'static str,
    /// SHA-256 of the file, lowercase hexadecimal.
    pub sha256: &'static str,
    /// Size of the file.
    pub bytes: u64,
    /// Licence of the model.
    pub license: &'static str,
    /// What to expect, shown in the interface (measured on 32 real pictures,
    /// one core of a desktop processor).
    pub hint: &'static str,
}

/// Models offered, the lightest first.
pub const MODELS: &[ModelInfo] = &[
    ModelInfo {
        id: "yolox_nano",
        name: "YOLOX-nano",
        file: "yolox_nano.onnx",
        url: "https://github.com/Megvii-BaseDetection/YOLOX/releases/download/0.1.1rc0/yolox_nano.onnx",
        sha256: "c789161ed43c8269fcd4e67c67eeeb4e80c622da2eb296a20bc6007bd18a0b7d",
        bytes: 3_659_407,
        license: "Apache-2.0",
        hint: "Le plus léger : ≈ 40 ms par image, ≈ 35 Mo de mémoire. Peu de fausses alertes, mais rate les sujets petits ou peu éclairés.",
    },
    ModelInfo {
        id: "yolox_tiny",
        name: "YOLOX-tiny",
        file: "yolox_tiny.onnx",
        url: "https://github.com/Megvii-BaseDetection/YOLOX/releases/download/0.1.1rc0/yolox_tiny.onnx",
        sha256: "427cc366d34e27ff7a03e2899b5e3671425c262ea2291f88bb942bc1cc70b0f7",
        bytes: 20_219_662,
        license: "Apache-2.0",
        hint: "≈ 100 ms par image, ≈ 70 Mo. Trouve plus de choses que nano, mais aussi plus de fausses personnes (un chien, un jardin vide).",
    },
    ModelInfo {
        id: "yolox_s",
        name: "YOLOX-s",
        file: "yolox_s.onnx",
        url: "https://github.com/Megvii-BaseDetection/YOLOX/releases/download/0.1.1rc0/yolox_s.onnx",
        sha256: "c5c2d13e59ae883e6af3b45daea64af4833a4951c92d116ec270d9ddbe998063",
        bytes: 35_858_002,
        license: "Apache-2.0",
        hint: "Le meilleur compromis : personne de nuit trouvée, chien reconnu, aucune fausse personne. ≈ 400 ms par image, ≈ 110 Mo.",
    },
    // The bigger ones: measured on 5 pictures, an i5-11600K (an older
    // processor such as an i5-6500 takes about 1.5 times longer).
    ModelInfo {
        id: "yolox_m",
        name: "YOLOX-m",
        file: "yolox_m.onnx",
        url: "https://github.com/Megvii-BaseDetection/YOLOX/releases/download/0.1.1rc0/yolox_m.onnx",
        sha256: "21ff6cfdeb53b013bac2249599e55f00bff3cfdfdab37ed7a4620818c1d15b3f",
        bytes: 101_259_744,
        license: "Apache-2.0",
        hint: "Plus juste que s (mAP 47,2 contre 40,5), plus sûr de lui sur les mêmes objets, mais 2,3 fois plus lent : ≈ 0,8 s par image (≈ 1,2 s sur un i5-6500), ≈ 240 Mo.",
    },
    ModelInfo {
        id: "yolox_l",
        name: "YOLOX-l",
        file: "yolox_l.onnx",
        url: "https://github.com/Megvii-BaseDetection/YOLOX/releases/download/0.1.1rc0/yolox_l.onnx",
        sha256: "7860ae79de6c89a3c1eb72ae9a2756c0ccfbe04b7791bb5880afabd97855a411",
        bytes: 216_746_733,
        license: "Apache-2.0",
        hint: "À peine plus juste que m (mAP 50,1) pour deux fois plus de calcul : ≈ 1,6 s par image (≈ 2,3 s sur un i5-6500), ≈ 440 Mo. Plutôt pour une machine puissante.",
    },
    ModelInfo {
        id: "yolox_darknet",
        name: "YOLOX-Darknet53",
        file: "yolox_darknet.onnx",
        url: "https://github.com/Megvii-BaseDetection/YOLOX/releases/download/0.1.1rc0/yolox_darknet.onnx",
        sha256: "10ccc527877c6ee5596929c4b3eee5ad6a78038647260bc9abdfce9a0df55d05",
        bytes: 254_814_364,
        license: "Apache-2.0",
        hint: "Plus lent que l et moins juste (mAP 48,0) : ≈ 2 s par image, ≈ 560 Mo. Gardé pour comparer ; l ou m font mieux.",
    },
    ModelInfo {
        id: "yolox_x",
        name: "YOLOX-x",
        file: "yolox_x.onnx",
        url: "https://github.com/Megvii-BaseDetection/YOLOX/releases/download/0.1.1rc0/yolox_x.onnx",
        sha256: "c892d7aaf1c4746d8a4d675bec669a4db4f434b4ee1efb654bc9b353379c7c55",
        bytes: 396_142_663,
        license: "Apache-2.0",
        hint: "Le plus juste (mAP 51,5) mais très lent sans carte graphique : ≈ 3,2 s par image (≈ 4,6 s sur un i5-6500), ≈ 800 Mo. Les analyses risquent de prendre du retard quand plusieurs caméras bougent.",
    },
];

/// A download taking longer is abandoned (YOLOX-x weighs 400 MB: 30 min
/// is about 2 Mbit/s).
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// The model `id`, if offered.
pub fn find(id: &str) -> Option<&'static ModelInfo> {
    MODELS.iter().find(|m| m.id == id)
}

/// SHA-256 of `data`, lowercase hexadecimal.
fn hex_sha256(digest: &[u8]) -> String {
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Whether `path` is the expected file of `model` (size and SHA-256).
/// Blocking.
pub fn is_downloaded(model: &ModelInfo, path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.len() == model.bytes)
        && std::fs::read(path).is_ok_and(|data| hex_sha256(&Sha256::digest(&data)) == model.sha256)
}

/// Downloads `model` into `dir` (created if needed), unless the right file
/// is already there, and returns its path. The file is written next to its
/// final name and only renamed once its size and SHA-256 are checked.
pub async fn download(model: &ModelInfo, dir: &Path) -> Result<PathBuf, String> {
    let path = dir.join(model.file);
    let present = path.clone();
    let info = *model;
    if tokio::task::spawn_blocking(move || is_downloaded(&info, &present))
        .await
        .unwrap_or(false)
    {
        return Ok(path);
    }
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| format!("impossible de créer {} : {e}", dir.display()))?;
    let part = dir.join(format!("{}.part", model.file));
    let result = tokio::time::timeout(DOWNLOAD_TIMEOUT, fetch(model, &part))
        .await
        .unwrap_or_else(|_| Err("téléchargement trop long, abandonné".to_owned()));
    if let Err(e) = result {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(e);
    }
    tokio::fs::rename(&part, &path)
        .await
        .map_err(|e| format!("impossible d'écrire {} : {e}", path.display()))?;
    Ok(path)
}

/// Writes `model` to `part`, checking its size and SHA-256.
async fn fetch(model: &ModelInfo, part: &Path) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;
    let mut response = client
        .get(model.url)
        .send()
        .await
        .map_err(|e| format!("téléchargement impossible (accès à Internet ?) : {e}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "téléchargement refusé par le serveur : {}",
            response.status()
        ));
    }
    let mut file = tokio::fs::File::create(part)
        .await
        .map_err(|e| format!("impossible d'écrire {} : {e}", part.display()))?;
    let mut hasher = Sha256::new();
    let mut received: u64 = 0;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| format!("téléchargement interrompu : {e}"))?
    {
        received += chunk.len() as u64;
        if received > model.bytes {
            return Err("fichier plus gros que prévu : refusé".to_owned());
        }
        hasher.update(&chunk);
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("impossible d'écrire {} : {e}", part.display()))?;
    }
    file.flush().await.map_err(|e| e.to_string())?;
    if received != model.bytes {
        return Err(format!(
            "fichier incomplet ({received} octets sur {})",
            model.bytes
        ));
    }
    if hex_sha256(&hasher.finalize()) != model.sha256 {
        return Err("empreinte SHA-256 inattendue : fichier refusé".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_the_size_and_the_fingerprint() {
        let data = b"not a model";
        let model = ModelInfo {
            id: "t",
            name: "t",
            file: "t.onnx",
            url: "http://127.0.0.1:9/t.onnx",
            sha256: "3a5a8b6dc6a0ff21b4ea4a0a0b7d4a0e2f0dbd4ef0d9e67a3e4bd2c6a5b1f6b1",
            bytes: data.len() as u64,
            license: "-",
            hint: "-",
        };
        let dir = std::env::temp_dir().join(format!("rustnvr-models-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("t.onnx");
        std::fs::write(&path, data).expect("write");
        // Right size, wrong fingerprint.
        assert!(!is_downloaded(&model, &path));
        let right = ModelInfo {
            sha256: Box::leak(hex_sha256(&Sha256::digest(data)).into_boxed_str()),
            ..model
        };
        assert!(is_downloaded(&right, &path));
        assert!(!is_downloaded(&right, &dir.join("absent.onnx")));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(find("yolox_nano").map(|m| m.file), Some("yolox_nano.onnx"));
        assert!(find("autre").is_none());
    }
}
