//! Dominant colour of a cover image: the average colour of its right-hand
//! 20% strip, sampled with ffmpeg (ported from `util/getDominantColor.ts`).

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

use crate::media::{Tool, file_input, lower_priority};

const SAMPLE_TIMEOUT: Duration = Duration::from_secs(30);

/// The dominant colour of the image at `path` as 6 lowercase hex digits. The
/// ffmpeg run happens in its own task, so it completes (or times out) even if
/// the caller stops waiting.
pub async fn dominant_color(path: &Path) -> Result<String, String> {
    let path: PathBuf = path.to_path_buf();
    let rgb = tokio::spawn(async move { sample_rgb(&path).await })
        .await
        .map_err(|error| format!("colour sampling task failed: {error}"))??;
    Ok(to_hex(rgb))
}

pub fn to_hex([red, green, blue]: [u8; 3]) -> String {
    format!("{red:02x}{green:02x}{blue:02x}")
}

async fn sample_rgb(path: &Path) -> Result<[u8; 3], String> {
    let mut command = Command::new(Tool::Ffmpeg.binary());
    lower_priority(&mut command);
    command
        .args([
            "-nostdin",
            "-v",
            "error",
            "-protocol_whitelist",
            "file",
            "-i",
        ])
        .arg(file_input(path))
        .args([
            "-vf",
            "crop=iw*0.2:ih:iw*0.8:0,scale=1:1:flags=area",
            "-frames:v",
            "1",
            "-pix_fmt",
            "rgb24",
            "-f",
            "rawvideo",
            "pipe:1",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);

    let output = match tokio::time::timeout(SAMPLE_TIMEOUT, command.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => return Err(format!("could not run ffmpeg: {error}")),
        Err(_) => return Err("colour sampling timed out".to_string()),
    };
    if !output.status.success() {
        return Err(format!("ffmpeg exited with {}", output.status));
    }
    match output.stdout.as_slice() {
        [red, green, blue, ..] => Ok([*red, *green, *blue]),
        _ => Err(format!("could not sample a colour from {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_rgb_as_hex() {
        assert_eq!(to_hex([0x5a, 0x24, 0x0a]), "5a240a");
        assert_eq!(to_hex([255, 255, 255]), "ffffff");
    }
}
