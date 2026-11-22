//! Tiny LAN web server so albums and settings can be managed from a phone instead of a TV remote.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

use crate::error::FrameError;
use crate::source::AlbumRef;

const PREFERRED_PORT: u16 = 8765;
const MAX_HEADER_BYTES: usize = 8 * 1024;
const MAX_BODY_BYTES: usize = 8 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(10);
const ACCEPT_POLL: Duration = Duration::from_millis(100);
const MAX_QUEUED: usize = 20;

const MSG_INVALID_LINK: &str = "That doesn't look like a shared album link. In Photos, open the shared album, \
     tap Share (or \u{2026}) \u{2192} Copy Link, then paste it here.";
const MSG_DUPLICATE: &str = "That album is already on the TV.";
const MSG_GONE: &str = "That album is no longer on the TV.";
const MSG_INVALID_SETTING: &str = "Please pick one of the listed options.";
const MSG_BUSY: &str = "The TV is busy. Please try again in a moment.";
const MSG_NOT_READY: &str = "The TV is still starting up. Please reload the page.";

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PhoneOption {
    pub value: u32,
    pub label: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct PhoneSettings {
    pub interval_seconds: u32,
    pub shuffle: bool,
    pub show_clock: bool,
    pub include_videos: bool,
    /// 0 = automatic.
    pub cache_mb: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PhoneAlbum {
    pub url: String,
    /// Empty until the album has been synced once.
    pub name: String,
    pub detail: String,
}

/// What the phone page shows; pushed by the app, which owns the real settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct PhoneState {
    pub albums: Vec<PhoneAlbum>,
    pub settings: PhoneSettings,
    pub interval_options: Vec<PhoneOption>,
    pub cache_options: Vec<PhoneOption>,
    pub sync_status: String,
    pub storage_status: String,
}

/// A validated change requested from the phone, for the app to apply.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum PhoneCommand {
    AddAlbum { url: String },
    RemoveAlbum { url: String },
    UpdateSettings { settings: PhoneSettings },
}

#[derive(Default)]
struct Shared {
    commands: VecDeque<PhoneCommand>,
    state: PhoneState,
}

type SharedState = Mutex<Shared>;

fn lock(shared: &SharedState) -> MutexGuard<'_, Shared> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(uniffi::Object)]
pub struct SetupServer {
    port: u16,
    secret: String,
    shared: Arc<SharedState>,
    stop: Arc<AtomicBool>,
}

#[uniffi::export]
impl SetupServer {
    /// Starts listening on all interfaces; only requests carrying the random secret path are served.
    #[uniffi::constructor]
    pub fn start() -> std::result::Result<Arc<Self>, FrameError> {
        let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, PREFERRED_PORT))
            .or_else(|_| TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0)))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();

        let mut bytes = [0u8; 16];
        getrandom::getrandom(&mut bytes).map_err(|e| FrameError::Storage(e.to_string()))?;
        let secret: String = bytes.iter().map(|b| format!("{b:02x}")).collect();

        let server = Arc::new(Self {
            port,
            secret: secret.clone(),
            shared: Arc::new(Mutex::new(Shared::default())),
            stop: Arc::new(AtomicBool::new(false)),
        });
        let shared = Arc::clone(&server.shared);
        let stop = Arc::clone(&server.stop);
        thread::Builder::new()
            .name("setup-server".into())
            .spawn(move || serve(listener, &secret, &shared, &stop))?;
        Ok(server)
    }

    /// URL to encode in the QR code, given the TV's LAN address.
    pub fn url(&self, host_ip: String) -> String {
        format!("http://{}:{}/s/{}", host_ip.trim(), self.port, self.secret)
    }

    /// Returns the next change requested from a phone, oldest first.
    pub fn take_command(&self) -> Option<PhoneCommand> {
        lock(&self.shared).commands.pop_front()
    }

    /// Ignored while commands are pending, so the phone never sees its change undone by a stale snapshot.
    pub fn set_state(&self, state: PhoneState) {
        let mut shared = lock(&self.shared);
        if shared.commands.is_empty() {
            shared.state = state;
        }
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for SetupServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn serve(listener: TcpListener, secret: &str, shared: &SharedState, stop: &AtomicBool) {
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                // Requests are tiny; handling them inline keeps the server single-threaded and bounded.
                let _ = handle(stream, secret, shared);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => thread::sleep(ACCEPT_POLL),
            Err(_) => thread::sleep(ACCEPT_POLL),
        }
    }
}

struct Request {
    method: String,
    path: String,
    body: Vec<u8>,
}

fn read_request(stream: &TcpStream) -> std::io::Result<Option<Request>> {
    let mut reader = BufReader::new(stream.take((MAX_HEADER_BYTES + MAX_BODY_BYTES) as u64));
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let (Some(method), Some(path)) = (parts.next(), parts.next()) else { return Ok(None) };
    let (method, path) = (method.to_string(), path.to_string());

    let mut content_length = 0usize;
    let mut header_bytes = line.len();
    loop {
        line.clear();
        let n = reader.read_line(&mut line)?;
        header_bytes += n;
        if n == 0 || header_bytes > MAX_HEADER_BYTES {
            return Ok(None);
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().unwrap_or(usize::MAX);
            }
        }
    }
    if content_length > MAX_BODY_BYTES {
        return Ok(None);
    }
    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body)?;
    Ok(Some(Request { method, path, body }))
}

fn handle(mut stream: TcpStream, secret: &str, shared: &SharedState) -> std::io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;

    let Some(req) = read_request(&stream)? else {
        return respond(&mut stream, "400 Bad Request", &page("Bad request", "", None));
    };
    let expected = format!("/s/{secret}");
    let (path, query) = req.path.split_once('?').unwrap_or((req.path.as_str(), ""));
    if path != expected {
        return respond(&mut stream, "404 Not Found", &page("Not found", "Scan the QR code shown on the TV.", None));
    }

    match req.method.as_str() {
        "GET" => {
            let notice = form_urlencoded::parse(query.as_bytes())
                .find(|(k, _)| k == "n")
                .and_then(|(_, v)| notice_text(&v));
            let state = lock(shared).state.clone();
            respond(&mut stream, "200 OK", &main_page(&expected, &state, notice, None))
        }
        "POST" => {
            let form: Vec<(String, String)> = form_urlencoded::parse(&req.body).into_owned().collect();
            match apply_post(&form, shared) {
                // Redirect so reloading the page doesn't resubmit the form.
                Ok(notice) => redirect(&mut stream, &format!("{expected}?n={notice}")),
                Err(error) => {
                    let state = lock(shared).state.clone();
                    respond(&mut stream, "200 OK", &main_page(&expected, &state, None, Some(error)))
                }
            }
        }
        _ => respond(&mut stream, "405 Method Not Allowed", &page("Not allowed", "", None)),
    }
}

/// Validates a form post, queues the command and mirrors it in the shown state; returns the notice key.
fn apply_post(form: &[(String, String)], shared: &SharedState) -> Result<&'static str, &'static str> {
    let field = |name: &str| form.iter().find(|(k, _)| k == name).map(|(_, v)| v.trim());
    let mut s = lock(shared);
    if s.commands.len() >= MAX_QUEUED {
        return Err(MSG_BUSY);
    }
    match field("action").unwrap_or("add") {
        "add" => {
            let url = field("album").unwrap_or_default();
            let key = AlbumRef::parse(url).map_err(|_| MSG_INVALID_LINK)?.key();
            if s.state.albums.iter().any(|a| AlbumRef::parse(&a.url).is_ok_and(|r| r.key() == key)) {
                return Err(MSG_DUPLICATE);
            }
            s.state.albums.push(PhoneAlbum {
                url: url.to_string(),
                name: String::new(),
                detail: "Added \u{2014} downloading on the TV\u{2026}".into(),
            });
            s.commands.push_back(PhoneCommand::AddAlbum { url: url.to_string() });
            Ok("added")
        }
        "remove" => {
            let url = field("url").unwrap_or_default();
            let before = s.state.albums.len();
            s.state.albums.retain(|a| a.url != url);
            if s.state.albums.len() == before {
                return Err(MSG_GONE);
            }
            s.commands.push_back(PhoneCommand::RemoveAlbum { url: url.to_string() });
            Ok("removed")
        }
        "settings" => {
            if s.state.interval_options.is_empty() || s.state.cache_options.is_empty() {
                return Err(MSG_NOT_READY);
            }
            let pick = |name: &str, options: &[PhoneOption]| -> Option<u32> {
                let value: u32 = field(name)?.parse().ok()?;
                options.iter().any(|o| o.value == value).then_some(value)
            };
            let settings = PhoneSettings {
                interval_seconds: pick("interval", &s.state.interval_options).ok_or(MSG_INVALID_SETTING)?,
                shuffle: field("shuffle").is_some(),
                show_clock: field("clock").is_some(),
                include_videos: field("videos").is_some(),
                cache_mb: pick("cache", &s.state.cache_options).ok_or(MSG_INVALID_SETTING)?,
            };
            s.state.settings = settings.clone();
            s.commands.push_back(PhoneCommand::UpdateSettings { settings });
            Ok("saved")
        }
        _ => Err(MSG_INVALID_SETTING),
    }
}

fn notice_text(key: &str) -> Option<&'static str> {
    match key {
        "added" => Some("Album added. Its photos will appear on the TV shortly."),
        "removed" => Some("Album removed from the TV."),
        "saved" => Some("Settings saved."),
        _ => None,
    }
}

fn respond(stream: &mut TcpStream, status: &str, html: &str) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nX-Frame-Options: DENY\r\n\
         Content-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; form-action 'self'\r\n\
         Connection: close\r\n\r\n{html}",
        html.len()
    )?;
    stream.flush()
}

/// `location` must be a server-built path, never user input.
fn redirect(stream: &mut TcpStream, location: &str) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 303 See Other\r\nLocation: {location}\r\nContent-Length: 0\r\n\
         Cache-Control: no-store\r\nConnection: close\r\n\r\n"
    )?;
    stream.flush()
}

fn escape(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '&' => "&amp;".into(),
            '<' => "&lt;".into(),
            '>' => "&gt;".into(),
            '"' => "&quot;".into(),
            '\'' => "&#39;".into(),
            c => c.to_string(),
        })
        .collect()
}

fn main_page(action: &str, state: &PhoneState, notice: Option<&str>, error: Option<&str>) -> String {
    let action = escape(action);
    let mut body = String::new();
    if let Some(n) = notice {
        body.push_str(&format!(r#"<p class="ok">{}</p>"#, escape(n)));
    }

    body.push_str("<h2>Albums on the TV</h2>");
    if state.albums.is_empty() {
        body.push_str(r#"<p class="muted">No albums yet. Add one below.</p>"#);
    }
    for album in &state.albums {
        let title = if album.name.is_empty() { &album.url } else { &album.name };
        body.push_str(&format!(
            r#"<div class="card"><div class="title">{title}</div><div class="muted">{detail}</div>
<details><summary>Remove&hellip;</summary><form method="post" action="{action}">
<input type="hidden" name="action" value="remove"><input type="hidden" name="url" value="{url}">
<button class="danger" type="submit">Remove from TV</button></form></details></div>"#,
            title = escape(title),
            detail = escape(&album.detail),
            url = escape(&album.url),
        ));
    }

    body.push_str(&format!(
        r#"<h2>Add a shared album</h2>
<ol>
<li>Open <b>Photos</b> on your iPhone and go to the shared album.</li>
<li>Tap <b>Share</b> (or <b>&hellip;</b>) &rarr; <b>Copy Link</b>.</li>
<li>Paste it below and tap <b>Send to TV</b>.</li>
</ol>
<form method="post" action="{action}">
<input type="hidden" name="action" value="add">
<input type="url" name="album" required placeholder="https://photos.icloud.com/shared/album/..." autocomplete="off">
<button type="submit">Send to TV</button>
</form>"#
    ));

    if !state.interval_options.is_empty() && !state.cache_options.is_empty() {
        let s = &state.settings;
        body.push_str(&format!(
            r#"<h2>Slideshow settings</h2>
<form method="post" action="{action}">
<input type="hidden" name="action" value="settings">
<label>Time per photo{interval}</label>
{shuffle}{clock}{videos}
<label>Storage for photos{cache}</label>
<button type="submit">Save settings</button>
</form>"#,
            interval = select("interval", &state.interval_options, s.interval_seconds),
            shuffle = checkbox("shuffle", "Shuffle", s.shuffle),
            clock = checkbox("clock", "Show clock and date", s.show_clock),
            videos = checkbox("videos", "Include videos and Live Photos", s.include_videos),
            cache = select("cache", &state.cache_options, s.cache_mb),
        ));
    }

    if !state.sync_status.is_empty() || !state.storage_status.is_empty() {
        body.push_str("<h2>Status</h2>");
        for line in [&state.sync_status, &state.storage_status] {
            if !line.is_empty() {
                body.push_str(&format!(r#"<p class="muted">{}</p>"#, escape(line)));
            }
        }
    }
    page("AirProject", &body, error)
}

fn select(name: &str, options: &[PhoneOption], selected: u32) -> String {
    let items: String = options
        .iter()
        .map(|o| {
            format!(
                r#"<option value="{}"{}>{}</option>"#,
                o.value,
                if o.value == selected { " selected" } else { "" },
                escape(&o.label)
            )
        })
        .collect();
    format!(r#"<select name="{}">{items}</select>"#, escape(name))
}

fn checkbox(name: &str, label: &str, checked: bool) -> String {
    format!(
        r#"<label class="check"><input type="checkbox" name="{}" value="1"{}> {}</label>"#,
        escape(name),
        if checked { " checked" } else { "" },
        escape(label)
    )
}

fn page(title: &str, body: &str, error: Option<&str>) -> String {
    let error = error
        .map(|e| format!(r#"<p class="err">{}</p>"#, escape(e)))
        .unwrap_or_default();
    format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1"><title>AirProject</title>
<style>
body{{font-family:-apple-system,system-ui,sans-serif;max-width:32rem;margin:2rem auto;padding:0 1.2rem;background:#111;color:#eee;line-height:1.5}}
h1{{font-size:1.5rem}} h2{{font-size:1.15rem;margin-top:2rem}} li{{margin:.4rem 0}} a{{color:#4aa3ff}}
input[type=url],select{{width:100%;box-sizing:border-box;font-size:1rem;padding:.8rem;border-radius:.6rem;border:1px solid #555;background:#222;color:#eee;margin-top:.3rem}}
label{{display:block;margin-top:1rem}} label.check{{display:flex;gap:.6rem;align-items:center}}
input[type=checkbox]{{width:1.3rem;height:1.3rem}}
button{{width:100%;margin-top:1rem;font-size:1.1rem;padding:.9rem;border:0;border-radius:.6rem;background:#0a84ff;color:#fff}}
button.danger{{background:#c0392b}}
.card{{background:#1c1c1e;border-radius:.6rem;padding:.8rem 1rem;margin:.6rem 0}} .title{{font-weight:600;word-break:break-all}}
.muted{{color:#9a9aa0}} summary{{color:#ff6b6b;margin-top:.4rem;cursor:pointer}}
.err{{background:#5a1d1d;padding:.8rem;border-radius:.6rem}} .ok{{background:#1d4a2a;padding:.8rem;border-radius:.6rem}}
</style></head><body><h1>{title}</h1>{error}{body}</body></html>"#,
        title = escape(title)
    )
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct QrMatrix {
    pub size: u32,
    /// Row-major, `true` = dark module.
    pub modules: Vec<bool>,
}

/// Encodes text as a QR code matrix for the TV to draw.
#[uniffi::export]
pub fn qr_matrix(text: String) -> std::result::Result<QrMatrix, FrameError> {
    let code = qrcode::QrCode::new(text.as_bytes()).map_err(|e| FrameError::Image(e.to_string()))?;
    let size = u32::try_from(code.width()).map_err(|e| FrameError::Image(e.to_string()))?;
    let modules = code
        .to_colors()
        .into_iter()
        .map(|c| c == qrcode::Color::Dark)
        .collect();
    Ok(QrMatrix { size, modules })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(port: u16, raw: &str) -> String {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all(raw.as_bytes()).unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        out
    }

    fn post(port: u16, path: &str, body: &str) -> String {
        request(port, &format!("POST {path} HTTP/1.1\r\nContent-Length: {}\r\n\r\n{body}", body.len()))
    }

    fn secret_path(server: &SetupServer) -> String {
        let url = server.url("127.0.0.1".into());
        url.split_once(&format!(":{}", server.port)).unwrap().1.to_string()
    }

    fn options(values: &[u32]) -> Vec<PhoneOption> {
        values.iter().map(|&value| PhoneOption { value, label: format!("{value}") }).collect()
    }

    #[test]
    fn accepts_valid_link_only_with_secret() {
        let server = SetupServer::start().unwrap();
        let path = secret_path(&server);

        let wrong = request(server.port, "GET /s/wrong HTTP/1.1\r\n\r\n");
        assert!(wrong.starts_with("HTTP/1.1 404"));

        let form = request(server.port, &format!("GET {path} HTTP/1.1\r\n\r\n"));
        assert!(form.starts_with("HTTP/1.1 200") && form.contains("<form"));

        let resp = post(server.port, &path, "album=%3Cscript%3E");
        assert!(!resp.contains("<script>"));
        assert!(resp.contains("doesn&#39;t look like a shared album link"));
        assert!(server.take_command().is_none());

        let good = "album=https%3A%2F%2Fphotos.icloud.com%2Fshared%2Falbum%2F0ExampleSharedAlbum123";
        let resp = post(server.port, &path, good);
        assert!(resp.starts_with("HTTP/1.1 303"));
        assert!(resp.contains(&format!("Location: {path}?n=added")));
        let dup = post(server.port, &path, &format!("action=add&{good}"));
        assert!(dup.contains("already on the TV"));
        post(server.port, &path, "action=add&album=https%3A%2F%2Fwww.icloud.com%2Fsharedalbum%2F%23B0aExampleAlbum1");

        let page = request(server.port, &format!("GET {path}?n=added HTTP/1.1\r\n\r\n"));
        assert!(page.contains("Album added"));
        assert!(page.contains("0ExampleSharedAlbum123"));

        assert_eq!(
            server.take_command(),
            Some(PhoneCommand::AddAlbum { url: "https://photos.icloud.com/shared/album/0ExampleSharedAlbum123".into() })
        );
        assert_eq!(
            server.take_command(),
            Some(PhoneCommand::AddAlbum { url: "https://www.icloud.com/sharedalbum/#B0aExampleAlbum1".into() })
        );
        assert!(server.take_command().is_none());
        server.stop();
    }

    #[test]
    fn removes_only_known_albums() {
        let server = SetupServer::start().unwrap();
        let path = secret_path(&server);
        let url = "https://www.icloud.com/sharedalbum/#B0aExampleAlbum1";
        server.set_state(PhoneState {
            albums: vec![PhoneAlbum { url: url.into(), name: "Family <3".into(), detail: "5 items".into() }],
            ..PhoneState::default()
        });

        let page = request(server.port, &format!("GET {path} HTTP/1.1\r\n\r\n"));
        assert!(page.contains("Family &lt;3"));

        let resp = post(server.port, &path, "action=remove&url=https%3A%2F%2Fexample.com");
        assert!(resp.contains("no longer on the TV"));
        assert!(server.take_command().is_none());

        let resp = post(server.port, &path, "action=remove&url=https%3A%2F%2Fwww.icloud.com%2Fsharedalbum%2F%23B0aExampleAlbum1");
        assert!(resp.starts_with("HTTP/1.1 303"));
        // A stale snapshot must not bring the album back while the command is pending.
        server.set_state(PhoneState {
            albums: vec![PhoneAlbum { url: url.into(), name: String::new(), detail: String::new() }],
            ..PhoneState::default()
        });
        let page = request(server.port, &format!("GET {path} HTTP/1.1\r\n\r\n"));
        assert!(!page.contains("B0aExampleAlbum1"));
        assert_eq!(server.take_command(), Some(PhoneCommand::RemoveAlbum { url: url.into() }));
        server.stop();
    }

    #[test]
    fn validates_settings_against_options() {
        let server = SetupServer::start().unwrap();
        let path = secret_path(&server);

        let resp = post(server.port, &path, "action=settings&interval=15&cache=0");
        assert!(resp.contains("still starting up"));

        server.set_state(PhoneState {
            interval_options: options(&[10, 15, 30]),
            cache_options: options(&[0, 500]),
            ..PhoneState::default()
        });
        let resp = post(server.port, &path, "action=settings&interval=7&cache=0");
        assert!(resp.contains("listed options"));
        assert!(server.take_command().is_none());

        let resp = post(server.port, &path, "action=settings&interval=30&cache=500&shuffle=1&videos=1");
        assert!(resp.starts_with("HTTP/1.1 303"));
        assert_eq!(
            server.take_command(),
            Some(PhoneCommand::UpdateSettings {
                settings: PhoneSettings {
                    interval_seconds: 30,
                    shuffle: true,
                    show_clock: false,
                    include_videos: true,
                    cache_mb: 500,
                }
            })
        );
        server.stop();
    }

    #[test]
    fn rejects_oversized_body() {
        let server = SetupServer::start().unwrap();
        let resp = request(server.port, "POST /s/x HTTP/1.1\r\nContent-Length: 999999\r\n\r\n");
        assert!(resp.starts_with("HTTP/1.1 400"));
        server.stop();
    }

    #[test]
    fn qr_has_square_matrix() {
        let qr = qr_matrix("http://192.168.0.5:8765/s/abc".into()).unwrap();
        assert_eq!(qr.modules.len(), (qr.size * qr.size) as usize);
        assert!(qr.modules.iter().any(|&m| m));
    }
}
