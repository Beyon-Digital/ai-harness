use std::io::{BufRead, Write};
use std::process::Command;
use std::time::Duration;

#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::path::Path;

use serde_json::{Value, json};

fn main() {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let Ok(message) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        let Some(id) = message.get("id").cloned() else {
            continue;
        };
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        match handle(method, &params) {
            Ok(result) => respond(&mut out, id, Some(result), None),
            Err(error) => respond(&mut out, id, None, Some((-32000, &error))),
        }
    }
}

fn handle(method: &str, params: &Value) -> Result<Value, String> {
    match method {
        "initialize" => Ok(json!({
            "protocolVersion": "2025-03-26",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "computer-mcp", "version": "0.1.0"},
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": tool_definitions()})),
        "tools/call" => call_tool(params),
        _ => Err(format!("method not found: {method}")),
    }
}

fn tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "screenshot",
            "description": "Capture the current desktop as a JPEG image.",
            "inputSchema": {"type": "object", "properties": {}},
        }),
        json!({
            "name": "click",
            "description": "Move the pointer to screen coordinates and click.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "x": {"type": "integer"},
                    "y": {"type": "integer"},
                    "button": {"type": "string", "enum": ["left", "middle", "right"]}
                },
                "required": ["x", "y"]
            },
        }),
        json!({
            "name": "type",
            "description": "Type text into the focused application.",
            "inputSchema": {
                "type": "object",
                "properties": {"text": {"type": "string"}},
                "required": ["text"]
            },
        }),
        json!({
            "name": "key",
            "description": "Press a key or shortcut such as ENTER, ESC, CTRL+L, or CMD+K.",
            "inputSchema": {
                "type": "object",
                "properties": {"key": {"type": "string"}},
                "required": ["key"]
            },
        }),
        json!({
            "name": "wait",
            "description": "Wait for the desktop to update.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "milliseconds": {"type": "integer", "minimum": 0, "maximum": 10000}
                }
            },
        }),
    ]
}

fn call_tool(params: &Value) -> Result<Value, String> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or("missing tool name")?;
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    match name {
        "screenshot" => {
            let image = screenshot()?;
            Ok(json!({
                "content": [
                    {"type": "text", "text": "Captured the current desktop."},
                    {"type": "image", "mimeType": "image/jpeg", "data": base64_encode(&image)}
                ],
                "isError": false
            }))
        }
        "click" => {
            let x = integer(&args, "x")?;
            let y = integer(&args, "y")?;
            let button = args.get("button").and_then(Value::as_str).unwrap_or("left");
            click(x, y, button)?;
            text_result(format!("Clicked {button} at {x}, {y}."))
        }
        "type" => {
            let text = string(&args, "text")?;
            type_text(text)?;
            text_result(format!("Typed {} characters.", text.chars().count()))
        }
        "key" => {
            let key = string(&args, "key")?;
            press_key(key)?;
            text_result(format!("Pressed {key}."))
        }
        "wait" => {
            let ms = args
                .get("milliseconds")
                .and_then(Value::as_u64)
                .unwrap_or(750)
                .min(10_000);
            std::thread::sleep(Duration::from_millis(ms));
            text_result(format!("Waited {ms} ms."))
        }
        _ => Err(format!("unknown tool: {name}")),
    }
}

fn integer(args: &Value, key: &str) -> Result<i64, String> {
    args.get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| format!("missing integer `{key}`"))
}

fn string<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing string `{key}`"))
}

fn text_result(text: String) -> Result<Value, String> {
    Ok(json!({
        "content": [{"type": "text", "text": text}],
        "isError": false
    }))
}

#[cfg(target_os = "linux")]
fn screenshot() -> Result<Vec<u8>, String> {
    run_output(
        "import",
        &[
            "-window",
            "root",
            "-resize",
            "1280x1280>",
            "-quality",
            "70",
            "jpg:-",
        ],
    )
}

#[cfg(target_os = "macos")]
fn screenshot() -> Result<Vec<u8>, String> {
    let path = std::env::temp_dir().join(format!("agentos-{}.jpg", std::process::id()));
    run_status("screencapture", &["-x", "-t", "jpg", path_str(&path)?])?;
    let bytes = std::fs::read(&path).map_err(|e| format!("read screenshot: {e}"))?;
    let _ = std::fs::remove_file(path);
    Ok(bytes)
}

#[cfg(target_os = "windows")]
fn screenshot() -> Result<Vec<u8>, String> {
    let path = std::env::temp_dir().join(format!("agentos-{}.jpg", std::process::id()));
    let quoted = ps_quote(path_str(&path)?);
    let script = format!(
        "Add-Type -AssemblyName System.Windows.Forms; Add-Type -AssemblyName System.Drawing; \
         $b=[System.Windows.Forms.Screen]::PrimaryScreen.Bounds; \
         $i=New-Object System.Drawing.Bitmap $b.Width,$b.Height; \
         $g=[System.Drawing.Graphics]::FromImage($i); \
         $g.CopyFromScreen($b.Location,[System.Drawing.Point]::Empty,$b.Size); \
         $i.Save('{quoted}',[System.Drawing.Imaging.ImageFormat]::Jpeg); \
         $g.Dispose(); $i.Dispose()"
    );
    run_status("powershell", &["-NoProfile", "-Command", &script])?;
    let bytes = std::fs::read(&path).map_err(|e| format!("read screenshot: {e}"))?;
    let _ = std::fs::remove_file(path);
    Ok(bytes)
}

#[cfg(target_os = "linux")]
fn click(x: i64, y: i64, button: &str) -> Result<(), String> {
    let button = match button {
        "middle" => "2",
        "right" => "3",
        _ => "1",
    };
    run_status(
        "xdotool",
        &[
            "mousemove",
            "--sync",
            &x.to_string(),
            &y.to_string(),
            "click",
            button,
        ],
    )
}

#[cfg(target_os = "macos")]
fn click(x: i64, y: i64, _button: &str) -> Result<(), String> {
    run_status(
        "osascript",
        &[
            "-e",
            &format!("tell application \"System Events\" to click at {{{x}, {y}}}"),
        ],
    )
}

#[cfg(target_os = "windows")]
fn click(x: i64, y: i64, button: &str) -> Result<(), String> {
    let (down, up) = match button {
        "right" => ("0x0008", "0x0010"),
        "middle" => ("0x0020", "0x0040"),
        _ => ("0x0002", "0x0004"),
    };
    let script = format!(
        "Add-Type @'\nusing System.Runtime.InteropServices; public class M {{ \
         [DllImport(\"user32.dll\")] public static extern bool SetCursorPos(int x,int y); \
         [DllImport(\"user32.dll\")] public static extern void mouse_event(uint f,uint x,uint y,uint d,System.UIntPtr e); }}\n'@; \
         [M]::SetCursorPos({x},{y}); [M]::mouse_event({down},0,0,0,[UIntPtr]::Zero); \
         [M]::mouse_event({up},0,0,0,[UIntPtr]::Zero)"
    );
    run_status("powershell", &["-NoProfile", "-Command", &script])
}

#[cfg(target_os = "linux")]
fn type_text(text: &str) -> Result<(), String> {
    run_status(
        "xdotool",
        &["type", "--clearmodifiers", "--delay", "1", "--", text],
    )
}

#[cfg(target_os = "macos")]
fn type_text(text: &str) -> Result<(), String> {
    run_status(
        "osascript",
        &[
            "-e",
            "on run argv",
            "-e",
            "tell application \"System Events\" to keystroke (item 1 of argv)",
            "-e",
            "end run",
            "--",
            text,
        ],
    )
}

#[cfg(target_os = "windows")]
fn type_text(text: &str) -> Result<(), String> {
    let encoded = base64_encode(text.as_bytes());
    let script = format!(
        "$t=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{encoded}')); \
         Set-Clipboard -Value $t; Add-Type -AssemblyName System.Windows.Forms; \
         [System.Windows.Forms.SendKeys]::SendWait('^v')"
    );
    run_status("powershell", &["-NoProfile", "-STA", "-Command", &script])
}

#[cfg(target_os = "linux")]
fn press_key(key: &str) -> Result<(), String> {
    run_status("xdotool", &["key", "--clearmodifiers", key])
}

#[cfg(target_os = "macos")]
fn press_key(key: &str) -> Result<(), String> {
    let normalized = key.to_ascii_uppercase();
    let mut parts = normalized.split('+').collect::<Vec<_>>();
    let value = parts.pop().ok_or("missing key")?;
    let code = mac_key_code(value)?;
    let modifiers = parts
        .into_iter()
        .map(mac_modifier)
        .collect::<Result<Vec<_>, _>>()?;
    let modifiers = if modifiers.is_empty() {
        String::new()
    } else {
        format!(
            " using {{{}}}",
            modifiers
                .iter()
                .map(|modifier| format!("{modifier} down"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    run_status(
        "osascript",
        &[
            "-e",
            &format!("tell application \"System Events\" to key code {code}{modifiers}"),
        ],
    )
}

#[cfg(target_os = "windows")]
fn press_key(key: &str) -> Result<(), String> {
    let key = match key.to_ascii_uppercase().as_str() {
        "ENTER" => "{ENTER}".to_owned(),
        "ESC" | "ESCAPE" => "{ESC}".to_owned(),
        "TAB" => "{TAB}".to_owned(),
        "BACKSPACE" => "{BACKSPACE}".to_owned(),
        "DELETE" => "{DELETE}".to_owned(),
        "ARROWLEFT" => "{LEFT}".to_owned(),
        "ARROWRIGHT" => "{RIGHT}".to_owned(),
        "ARROWUP" => "{UP}".to_owned(),
        "ARROWDOWN" => "{DOWN}".to_owned(),
        other => other
            .replace("CTRL+", "^")
            .replace("ALT+", "%")
            .replace("SHIFT+", "+"),
    };
    let script = format!(
        "Add-Type -AssemblyName System.Windows.Forms; \
         [System.Windows.Forms.SendKeys]::SendWait('{}')",
        ps_quote(&key)
    );
    run_status("powershell", &["-NoProfile", "-Command", &script])
}

#[cfg(any(target_os = "macos", test))]
fn mac_key_code(key: &str) -> Result<u8, String> {
    let code = match key {
        "A" => 0,
        "S" => 1,
        "D" => 2,
        "F" => 3,
        "H" => 4,
        "G" => 5,
        "Z" => 6,
        "X" => 7,
        "C" => 8,
        "V" => 9,
        "B" => 11,
        "Q" => 12,
        "W" => 13,
        "E" => 14,
        "R" => 15,
        "Y" => 16,
        "T" => 17,
        "1" => 18,
        "2" => 19,
        "3" => 20,
        "4" => 21,
        "6" => 22,
        "5" => 23,
        "9" => 25,
        "7" => 26,
        "8" => 28,
        "0" => 29,
        "O" => 31,
        "U" => 32,
        "I" => 34,
        "P" => 35,
        "L" => 37,
        "J" => 38,
        "K" => 40,
        "N" => 45,
        "M" => 46,
        "ENTER" => 36,
        "TAB" => 48,
        "SPACE" => 49,
        "BACKSPACE" => 51,
        "ESC" | "ESCAPE" => 53,
        "ARROWLEFT" => 123,
        "ARROWRIGHT" => 124,
        "ARROWDOWN" => 125,
        "ARROWUP" => 126,
        "F1" => 122,
        "F2" => 120,
        "F3" => 99,
        "F4" => 118,
        "F5" => 96,
        "F6" => 97,
        "F7" => 98,
        "F8" => 100,
        "F9" => 101,
        "F10" => 109,
        "F11" => 103,
        "F12" => 111,
        _ => return Err(format!("unsupported macOS key: {key}")),
    };
    Ok(code)
}

#[cfg(any(target_os = "macos", test))]
fn mac_modifier(key: &str) -> Result<&'static str, String> {
    match key {
        "CMD" | "COMMAND" => Ok("command"),
        "ALT" | "OPTION" => Ok("option"),
        "SHIFT" => Ok("shift"),
        "CTRL" | "CONTROL" => Ok("control"),
        _ => Err(format!("unsupported macOS modifier: {key}")),
    }
}

fn run_output(program: &str, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| format!("{program}: {e}"))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(format!(
            "{program}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn run_status(program: &str, args: &[&str]) -> Result<(), String> {
    run_output(program, args).map(|_| ())
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn path_str(path: &Path) -> Result<&str, String> {
    path.to_str()
        .ok_or_else(|| "temporary path is not UTF-8".to_owned())
}

#[cfg(target_os = "windows")]
fn ps_quote(value: &str) -> String {
    value.replace('\'', "''")
}

fn base64_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let bytes = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let value = (u32::from(bytes[0]) << 16) | (u32::from(bytes[1]) << 8) | u32::from(bytes[2]);
        output.push(ALPHABET[((value >> 18) & 63) as usize] as char);
        output.push(ALPHABET[((value >> 12) & 63) as usize] as char);
        output.push(if chunk.len() > 1 {
            ALPHABET[((value >> 6) & 63) as usize] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            ALPHABET[(value & 63) as usize] as char
        } else {
            '='
        });
    }
    output
}

fn respond(out: &mut impl Write, id: Value, result: Option<Value>, error: Option<(i64, &str)>) {
    let response = match (result, error) {
        (Some(result), _) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        (_, Some((code, message))) => {
            json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
        }
        _ => return,
    };
    let mut bytes = serde_json::to_vec(&response).unwrap_or_default();
    bytes.push(b'\n');
    let _ = out.write_all(&bytes);
    let _ = out.flush();
}

#[cfg(test)]
mod tests {
    use super::{mac_key_code, mac_modifier};

    #[test]
    fn macos_shortcut_keys_cover_letters_digits_and_functions() {
        assert_eq!(mac_key_code("K"), Ok(40));
        assert_eq!(mac_key_code("7"), Ok(26));
        assert_eq!(mac_key_code("F12"), Ok(111));
        assert!(mac_key_code("UNKNOWN").is_err());
    }

    #[test]
    fn macos_shortcut_modifiers_reject_unknown_values() {
        assert_eq!(mac_modifier("CMD"), Ok("command"));
        assert_eq!(mac_modifier("CTRL"), Ok("control"));
        assert!(mac_modifier("META").is_err());
    }
}
