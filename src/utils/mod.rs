//! 实用工具模块
//!
//! 包含跨平台 home 目录解析和模型 ID 解析等辅助函数。

pub mod model_parser;

use std::env;
use std::path::PathBuf;

const VERTICAL_BLOCKS: &[char] = &['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

#[must_use]
pub fn pct_to_vertical_block(pct: f64) -> char {
    let clamped = pct.clamp(0.0, 100.0);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let idx = ((clamped / 100.0) * 7.0).round() as usize;
    VERTICAL_BLOCKS[idx.min(7)]
}

#[must_use]
pub fn rainbow_gradient_color(percentage: f64) -> (u8, u8, u8) {
    let p = percentage.clamp(0.0, 100.0);

    let soft_green = (80.0, 200.0, 80.0);
    let soft_yellow_green = (150.0, 200.0, 60.0);
    let soft_yellow = (200.0, 200.0, 80.0);
    let soft_orange = (220.0, 160.0, 60.0);
    let soft_red = (200.0, 100.0, 80.0);

    let lerp = |start: (f64, f64, f64), end: (f64, f64, f64), t: f64| {
        let clamp_t = t.clamp(0.0, 1.0);
        (
            (end.0 - start.0).mul_add(clamp_t, start.0),
            (end.1 - start.1).mul_add(clamp_t, start.1),
            (end.2 - start.2).mul_add(clamp_t, start.2),
        )
    };

    let (r, g, b) = if p <= 25.0 {
        lerp(soft_green, soft_yellow_green, p / 25.0)
    } else if p <= 50.0 {
        lerp(soft_yellow_green, soft_yellow, (p - 25.0) / 25.0)
    } else if p <= 75.0 {
        lerp(soft_yellow, soft_orange, (p - 50.0) / 25.0)
    } else {
        lerp(soft_orange, soft_red, (p - 75.0) / 25.0)
    };

    let convert = |value: f64| -> u8 {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        {
            value.clamp(0.0, 255.0).round() as u8
        }
    };

    (convert(r), convert(g), convert(b))
}

/// 获取用户主目录，优先尊重 `HOME` 环境变量。
///
/// 在 Windows Runner 上，GitHub Actions 会为子进程注入 `HOME`，但
/// [`dirs::home_dir`] 默认忽略该变量，导致 CLI 测试无法将配置写入预期路径。
/// 该辅助函数通过显式检查相关环境变量，保持与测试环境及类 Unix 行为的一致性。
#[must_use]
pub fn home_dir() -> Option<PathBuf> {
    if let Some(home) = env::var_os("HOME") {
        if !home.is_empty() {
            return Some(PathBuf::from(home));
        }
    }

    #[cfg(target_os = "windows")]
    {
        if let Some(profile) = env::var_os("USERPROFILE") {
            if !profile.is_empty() {
                return Some(PathBuf::from(profile));
            }
        }

        let drive = env::var_os("HOMEDRIVE");
        let path = env::var_os("HOMEPATH");
        if let (Some(drive), Some(path)) = (drive, path) {
            if !drive.is_empty() && !path.is_empty() {
                let mut combined = PathBuf::from(drive);
                combined.push(path);
                return Some(combined);
            }
        }
    }

    dirs::home_dir()
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{anyhow, Result};
    use std::ffi::OsString;
    use tempfile::tempdir;

    #[test]
    fn respects_home_env_when_present() -> Result<()> {
        let dir = tempdir()?;
        let original = env::var_os("HOME");
        env::set_var("HOME", dir.path());

        let detected = home_dir().ok_or_else(|| anyhow!("home dir unavailable"))?;
        assert_eq!(detected, dir.path());

        match original {
            Some(val) => env::set_var("HOME", val),
            None => env::remove_var("HOME"),
        }

        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn falls_back_to_dirs_home_dir() {
        let original_home = env::var_os("HOME");
        let original_profile = env::var_os("USERPROFILE");
        let original_drive = env::var_os("HOMEDRIVE");
        let original_path = env::var_os("HOMEPATH");

        env::remove_var("HOME");
        env::remove_var("USERPROFILE");
        env::remove_var("HOMEDRIVE");
        env::remove_var("HOMEPATH");

        let detected = home_dir();
        let expected = dirs::home_dir();

        // 在某些 CI 环境中，即使移除环境变量，dirs::home_dir() 仍可能
        // 通过系统调用（如读取 /etc/passwd）返回值，因此两者应该一致
        assert_eq!(
            detected, expected,
            "home_dir() should match dirs::home_dir() when env vars are removed"
        );

        restore_env("HOME", original_home);
        restore_env("USERPROFILE", original_profile);
        restore_env("HOMEDRIVE", original_drive);
        restore_env("HOMEPATH", original_path);
    }

    fn restore_env(key: &str, value: Option<OsString>) {
        if let Some(val) = value {
            env::set_var(key, val);
        } else {
            env::remove_var(key);
        }
    }
}
