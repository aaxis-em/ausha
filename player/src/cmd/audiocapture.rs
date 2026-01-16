use std::process::Command;

#[cfg(target_os = "windows")]
pub fn get_audio_capture_command() -> Vec<String> {
    vec![]
}

#[cfg(target_os = "linux")]
pub fn get_audio_capture_command() -> Vec<String> {
    let output = Command::new("pactl")
        .args(["list", "sources", "short"])
        .output()
        .expect("failed to execute pactl");

    if !output.status.success() {
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        return vec![];
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut source_name = None;

    for line in stdout.lines() {
        if line.contains("RUNNING") && line.contains("sink") {
            if let Some(name) = line.split_whitespace().nth(1) {
                source_name = Some(name.to_string());
                break;
            }
        }
    }

    let source_name = match source_name {
        Some(name) => name,
        None => {
            eprintln!("No running sink found");
            return vec![];
        }
    };

    vec![
        "-f".to_string(),
        "pulse".to_string(),
        "-i".to_string(),
        source_name,
        "-c:a".to_string(),
        "aac".to_string(),
        "-b:a".to_string(),
        "128k".to_string(),
        "-ar".to_string(),
        "48000".to_string(),
        "-ac".to_string(),
        "2".to_string(),
        "-f".to_string(),
        "mpegts".to_string(),
        "-".to_string(),
    ]
}
