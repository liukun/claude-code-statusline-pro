//! Quota component
//!
//! Displays API usage rate limits (5-hour and 7-day windows) fetched from
//! the Anthropic OAuth API.

use std::fmt::Write;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::base::{Component, ComponentFactory, ComponentOutput, RenderContext};
use crate::config::{BaseComponentConfig, Config, QuotaComponentConfig};
use crate::themes::{ansi_fg, ANSI_RESET};
use crate::utils::{pct_to_vertical_block, rainbow_gradient_color};

const OAUTH_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";

// ---------------------------------------------------------------------------
// API response models
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize)]
struct RateWindow {
    utilization: f64,
    resets_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct OAuthUsageResponse {
    five_hour: Option<RateWindow>,
    seven_day: Option<RateWindow>,
}

// ---------------------------------------------------------------------------
// Cache
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize)]
struct QuotaCache {
    fetched_at: u64,
    response: OAuthUsageResponse,
}

fn cache_path() -> PathBuf {
    crate::utils::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".claude")
        .join(".statusline-cache")
        .join("quota.json")
}

fn read_cache(ttl_secs: u64) -> Option<OAuthUsageResponse> {
    let data = std::fs::read_to_string(cache_path()).ok()?;
    let cache: QuotaCache = serde_json::from_str(&data).ok()?;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    if now.saturating_sub(cache.fetched_at) <= ttl_secs {
        Some(cache.response)
    } else {
        None
    }
}

fn write_cache(response: &OAuthUsageResponse) {
    let path = cache_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Serialize directly from reference — no clone needed
    let wrapper = serde_json::json!({
        "fetched_at": now,
        "response": response,
    });
    if let Ok(json) = serde_json::to_string(&wrapper) {
        let _ = std::fs::write(&path, json);
    }
}

// ---------------------------------------------------------------------------
// Credential helpers
// ---------------------------------------------------------------------------

fn extract_oauth_token(json: &serde_json::Value) -> Option<String> {
    json.get("claudeAiOauth")
        .and_then(|o| o.get("accessToken"))
        .and_then(serde_json::Value::as_str)
        .map(String::from)
}

fn read_access_token() -> Option<String> {
    if cfg!(target_os = "macos") {
        if let Some(token) = read_token_from_keychain() {
            return Some(token);
        }
    }
    read_token_from_credentials_file()
}

fn read_token_from_keychain() -> Option<String> {
    let output = Command::new("security")
        .args(["find-generic-password", "-s", KEYCHAIN_SERVICE, "-w"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let raw = String::from_utf8(output.stdout).ok()?;
    let json: serde_json::Value = serde_json::from_str(raw.trim()).ok()?;
    extract_oauth_token(&json)
}

fn read_token_from_credentials_file() -> Option<String> {
    let path = crate::utils::home_dir()?.join(".claude/.credentials.json");
    let data = std::fs::read_to_string(path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&data).ok()?;
    extract_oauth_token(&json)
}

// ---------------------------------------------------------------------------
// HTTP fetch
// ---------------------------------------------------------------------------

fn fetch_usage(token: &str) -> Option<OAuthUsageResponse> {
    let resp = ureq::get(OAUTH_USAGE_URL)
        .set("Authorization", &format!("Bearer {token}"))
        .set("anthropic-beta", "oauth-2025-04-20")
        .timeout(Duration::from_secs(5))
        .call()
        .ok()?;

    resp.into_json::<OAuthUsageResponse>().ok()
}

// ---------------------------------------------------------------------------
// Progress bar rendering
// ---------------------------------------------------------------------------

fn build_bar(percentage: f64, width: usize, supports_colors: bool) -> String {
    let clamped = percentage.clamp(0.0, 100.0);

    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    let filled = ((clamped / 100.0) * width as f64).round() as usize;
    let filled = filled.min(width);

    let mut bar = String::with_capacity(width * 16);

    for idx in 0..width {
        if idx < filled {
            if supports_colors {
                let (r, g, b) = gradient_color(clamped, idx, filled);
                let _ = write!(bar, "\x1b[38;2;{r};{g};{b}m\u{2588}");
            } else {
                bar.push('\u{2588}'); // █
            }
        } else if supports_colors {
            bar.push_str("\x1b[38;2;120;120;120m\u{2591}"); // ░
        } else {
            bar.push('\u{2591}'); // ░
        }
    }

    if supports_colors {
        bar.push_str("\x1b[0m");
    }

    bar
}

fn gradient_color(overall_pct: f64, idx: usize, filled: usize) -> (u8, u8, u8) {
    #[allow(clippy::cast_precision_loss)]
    let pos_pct = if filled == 0 {
        0.0
    } else {
        ((idx as f64 + 0.5) / filled as f64) * overall_pct
    }
    .clamp(0.0, 100.0);

    // Green → Yellow → Orange → Red
    let (r, g, b) = if pos_pct <= 33.0 {
        let t = pos_pct / 33.0;
        lerp((80.0, 200.0, 80.0), (200.0, 200.0, 80.0), t)
    } else if pos_pct <= 66.0 {
        let t = (pos_pct - 33.0) / 33.0;
        lerp((200.0, 200.0, 80.0), (220.0, 160.0, 60.0), t)
    } else {
        let t = (pos_pct - 66.0) / 34.0;
        lerp((220.0, 160.0, 60.0), (200.0, 80.0, 80.0), t)
    };

    let clamp = |v: f64| -> u8 {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        {
            v.clamp(0.0, 255.0).round() as u8
        }
    };

    (clamp(r), clamp(g), clamp(b))
}

fn lerp(a: (f64, f64, f64), b: (f64, f64, f64), t: f64) -> (f64, f64, f64) {
    let t = t.clamp(0.0, 1.0);
    (
        (b.0 - a.0).mul_add(t, a.0),
        (b.1 - a.1).mul_add(t, a.1),
        (b.2 - a.2).mul_add(t, a.2),
    )
}

/// Calculate how much of the rate window has elapsed, as a percentage.
///
/// `label` is `"5h"` or `"7d"`. `resets_at` is the ISO-8601 timestamp when
/// the window resets. Returns `None` when the timestamp is missing or unparseable.
fn elapsed_pct(label: &str, resets_at: Option<&str>) -> Option<f64> {
    let window_secs: f64 = match label {
        "5h" => 5.0 * 3600.0,
        "7d" => 7.0 * 24.0 * 3600.0,
        _ => return None,
    };

    let resets_str = resets_at?;
    let resets = chrono::DateTime::parse_from_rfc3339(resets_str).ok()?;
    let now = chrono::Utc::now();

    #[allow(clippy::cast_precision_loss)]
    let remaining = resets.signed_duration_since(now).num_seconds() as f64;
    let elapsed = window_secs - remaining;
    Some((elapsed / window_secs * 100.0).clamp(0.0, 100.0))
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

pub struct QuotaComponent {
    config: QuotaComponentConfig,
}

impl QuotaComponent {
    #[must_use]
    pub const fn new(config: QuotaComponentConfig) -> Self {
        Self { config }
    }

    fn get_usage(&self) -> Option<OAuthUsageResponse> {
        // Try cache first
        if let Some(cached) = read_cache(self.config.cache_ttl) {
            return Some(cached);
        }

        // Fetch from API
        let token = read_access_token()?;
        let response = fetch_usage(&token)?;
        write_cache(&response);
        Some(response)
    }

    fn render_window(&self, label: &str, window: &RateWindow, supports_colors: bool) -> String {
        let pct = window.utilization.clamp(0.0, 100.0);

        let mut text = String::new();

        text.push_str(label);

        if self.config.show_progress_bar {
            let width = self.config.progress_width.max(1) as usize;
            let bar = build_bar(pct, width, supports_colors);
            let _ = write!(text, "[{bar}]");
        }

        if self.config.show_percentage {
            let elapsed = elapsed_pct(label, window.resets_at.as_deref());

            let usage_str = if supports_colors {
                let (r, g, b) = rainbow_gradient_color(pct);
                if self.config.compact_bar {
                    let block = pct_to_vertical_block(pct);
                    format!("\x1b[38;2;{r};{g};{b}m{pct:.0}%{block}\x1b[0m")
                } else {
                    format!("\x1b[38;2;{r};{g};{b}m{pct:.0}%\x1b[0m")
                }
            } else if self.config.compact_bar {
                let block = pct_to_vertical_block(pct);
                format!("{pct:.0}%{block}")
            } else {
                format!("{pct:.0}%")
            };

            if let Some(ep) = elapsed {
                let (cmp, cmp_color) = if pct > ep + 1.0 {
                    (">", "red")
                } else if pct + 1.0 < ep {
                    ("<", "green")
                } else {
                    ("=", "yellow")
                };
                let cmp_str = if supports_colors {
                    ansi_fg(cmp_color).map_or_else(
                        || cmp.to_string(),
                        |ansi| format!("{ansi}{cmp}{ANSI_RESET}"),
                    )
                } else {
                    cmp.to_string()
                };
                if self.config.compact_bar {
                    let elapsed_block = pct_to_vertical_block(ep);
                    let _ = write!(text, "({usage_str}{cmp_str}{ep:.0}%{elapsed_block})");
                } else {
                    let _ = write!(text, "({usage_str}{cmp_str}{ep:.0}%)");
                }
            } else {
                let _ = write!(text, " {usage_str}");
            }
        }

        text
    }
}

#[async_trait]
impl Component for QuotaComponent {
    fn name(&self) -> &'static str {
        "quota"
    }

    fn is_enabled(&self, _ctx: &RenderContext) -> bool {
        self.config.base.enabled
    }

    fn base_config(&self, _ctx: &RenderContext) -> Option<&BaseComponentConfig> {
        Some(&self.config.base)
    }

    async fn render(&self, ctx: &RenderContext) -> ComponentOutput {
        let usage = tokio::task::spawn_blocking({
            let config = self.config.clone();
            move || Self::new(config).get_usage()
        })
        .await
        .ok()
        .flatten();

        let Some(usage) = usage else {
            return ComponentOutput::hidden();
        };

        let supports_colors = ctx.terminal.supports_colors();
        let mut segments = Vec::new();

        if self.config.show_five_hour {
            if let Some(ref w) = usage.five_hour {
                segments.push(self.render_window("5h", w, supports_colors));
            }
        }

        if self.config.show_seven_day {
            if let Some(ref w) = usage.seven_day {
                segments.push(self.render_window("7d", w, supports_colors));
            }
        }

        if segments.is_empty() {
            return ComponentOutput::hidden();
        }

        let text = segments.join(" ");
        let icon = self.select_icon(ctx);

        ComponentOutput::new(text).with_icon(icon.unwrap_or_default())
    }
}

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

pub struct QuotaComponentFactory;

impl ComponentFactory for QuotaComponentFactory {
    fn create(&self, config: &Config) -> Box<dyn Component> {
        Box::new(QuotaComponent::new(config.components.quota.clone()))
    }

    fn name(&self) -> &'static str {
        "quota"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_bar_empty() {
        let bar = build_bar(0.0, 8, false);
        assert_eq!(bar.chars().count(), 8);
        assert!(bar.chars().all(|c| c == '\u{2591}'));
    }

    #[test]
    fn test_build_bar_full() {
        let bar = build_bar(100.0, 8, false);
        assert_eq!(bar.chars().count(), 8);
        assert!(bar.chars().all(|c| c == '\u{2588}'));
    }

    #[test]
    fn test_build_bar_half() {
        let bar = build_bar(50.0, 8, false);
        let chars: Vec<char> = bar.chars().collect();
        assert_eq!(chars.len(), 8);
        assert_eq!(chars.iter().filter(|&&c| c == '\u{2588}').count(), 4);
    }

    #[test]
    fn test_gradient_color_does_not_panic() {
        // Just verify no panics across full range
        let _ = gradient_color(0.0, 0, 1);
        let _ = gradient_color(50.0, 3, 8);
        let _ = gradient_color(100.0, 7, 8);
    }

    type TestResult = anyhow::Result<()>;

    #[test]
    fn test_cache_roundtrip() -> TestResult {
        let response = OAuthUsageResponse {
            five_hour: Some(RateWindow {
                utilization: 42.0,
                resets_at: Some("2026-03-24T10:00:00Z".to_string()),
            }),
            seven_day: Some(RateWindow {
                utilization: 15.0,
                resets_at: None,
            }),
        };
        let cache = QuotaCache {
            fetched_at: 9_999_999_999,
            response,
        };
        let json = serde_json::to_string(&cache)?;
        let parsed: QuotaCache = serde_json::from_str(&json)?;
        let five = parsed
            .response
            .five_hour
            .ok_or_else(|| anyhow::anyhow!("missing five_hour"))?;
        let seven = parsed
            .response
            .seven_day
            .ok_or_else(|| anyhow::anyhow!("missing seven_day"))?;
        assert!((five.utilization - 42.0).abs() < f64::EPSILON);
        assert!((seven.utilization - 15.0).abs() < f64::EPSILON);
        Ok(())
    }
}
