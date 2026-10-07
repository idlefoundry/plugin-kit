//! The update check, made when the user asks (`docs/decisions.md` K3; the CA-72's R27, where its
//! owner chose a check made when the user asks, and a DOWNLOAD that opens the installer in the
//! browser). A plug-in shows it in its presets' drawer: its name and version and CHECK FOR
//! UPDATES. A press asks GitHub's API for the plug-in's repository's latest release through the
//! system's curl (nothing here links network code of its own), which writes the answer into a
//! file of the check's. The editor looks each frame whether curl has finished ([`Update::tick`]),
//! so no thread of the plug-in waits for it (none may run once the host has unloaded the
//! plug-in), and dropping the check (the editor closed) ends curl. A newer release offers
//! DOWNLOAD, which opens its installer for this system in the browser (the release's page if it
//! has none), for the user to run with the DAW closed; a check that failed offers the releases'
//! page. Nothing is checked unasked, and nothing is sent but the request.
//!
//! What a plug-in gives it is its [`App`]: its name as its release assets begin
//! (`<NAME>-<version>-macOS.pkg`, `-Windows-setup.exe`, `-Linux-x86_64.tar.gz`: a contract with
//! every installed copy), its version and its repository on GitHub. What the drawer shows is a
//! [`Scene`]; the plug-in draws it.

use std::fs::File;
use std::io::{self, Read};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

/// The plug-in whose releases are checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct App {
    /// Its name as its release assets and its user agent have it: `CA-72`.
    pub name: &'static str,
    /// This build's version: `env!("CARGO_PKG_VERSION")`.
    pub version: &'static str,
    /// Its repository: `https://github.com/idlefoundry/ca-72`.
    pub repository: &'static str,
}

impl App {
    /// The repository's releases' page; the browser is sent only to it and to pages under it.
    pub fn releases(&self) -> String {
        format!("{}/releases", self.repository.trim_end_matches('/'))
    }

    /// The repository's latest release as GitHub's API gives it (published releases only: no
    /// drafts, no pre-releases).
    pub fn latest(&self) -> String {
        let repository = self.repository.trim_end_matches('/');
        let path = repository
            .strip_prefix("https://github.com/")
            .unwrap_or(repository);
        format!("https://api.github.com/repos/{path}/releases/latest")
    }

    /// What curl calls itself: `CA-72/0.1.3`.
    pub fn agent(&self) -> String {
        format!("{}/{}", self.name, self.version)
    }

    /// The name of release `version`'s asset `suffix`: `CA-72-0.2.0-macOS.pkg`.
    fn asset(&self, version: &str, suffix: &str) -> String {
        format!("{}-{version}-{suffix}", self.name)
    }
}

/// How long curl may take, in seconds; the editor ends it if it runs on past `PATIENCE`.
const CURL_SECONDS: u32 = 20;
const PATIENCE: Duration = Duration::from_secs(30);
/// The most the answer may be, in bytes (the CA-72 0.1.0's was 15 kB).
const MOST: u64 = 1 << 20;

/// What follows `<NAME>-<version>-` in the name of the installer for this system (none where
/// no installer is made). The plug-ins' release jobs name their assets so.
pub const INSTALLER: Option<&str> = if cfg!(target_os = "windows") {
    Some("Windows-setup.exe")
} else if cfg!(target_os = "macos") {
    Some("macOS.pkg")
} else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
    Some("Linux-x86_64.tar.gz")
} else {
    None
};

/// A version's three numbers: `0.2.0`'s (none for anything else, a pre-release among it).
pub fn numbers(version: &str) -> Option<(u64, u64, u64)> {
    let mut n = version.split('.').map(|p| {
        if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        p.parse().ok()
    });
    let v = (n.next()??, n.next()??, n.next()??);
    n.next().is_none().then_some(v)
}

/// Whether `version` is later than `than`.
pub fn newer(version: &str, than: &str) -> bool {
    numbers(version) > numbers(than)
}

/// The latest release, as the drawer offers it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    /// Its version: its tag without the `v`.
    pub version: String,
    /// What DOWNLOAD opens: its installer for this system, else its page.
    pub download: String,
}

/// The release in the API's answer, its installer `<NAME>-<version>-<installer>`.
fn release(app: &App, answer: &[u8], installer: Option<&str>) -> Option<Release> {
    let releases = app.releases();
    let v: serde_json::Value = serde_json::from_slice(answer).ok()?;
    let tag = v.get("tag_name")?.as_str()?;
    let version = tag.strip_prefix('v').unwrap_or(tag);
    numbers(version)?;
    let page = v
        .get("html_url")
        .and_then(serde_json::Value::as_str)
        .filter(|u| ours(&releases, u))
        .unwrap_or(&releases);
    let name = installer.map(|i| app.asset(version, i));
    let download = v
        .get("assets")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .find(|a| {
            name.is_some() && a.get("name").and_then(serde_json::Value::as_str) == name.as_deref()
        })
        .and_then(|a| a.get("browser_download_url")?.as_str())
        .filter(|u| ours(&releases, u))
        .unwrap_or(page);
    Some(Release {
        version: version.to_owned(),
        download: download.to_owned(),
    })
}

/// Whether `url` is the repository's releases' page (`releases`) or one under it: the browser
/// is sent nowhere else, whatever an answer says.
fn ours(releases: &str, url: &str) -> bool {
    url.strip_prefix(releases).is_some_and(|rest| {
        (rest.is_empty() || rest.starts_with('/'))
            && !rest.contains("..")
            && rest
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-._~/".contains(&b))
    })
}

/// What starts curl asking for `app`'s latest release, its answer to `out` (the tests start
/// something else).
pub type Fetch = fn(app: &App, out: File) -> io::Result<Child>;
/// What opens a page in the browser (the tests open none): the process it started, if any, to
/// be reaped once it ends.
pub type Open = fn(url: &str) -> io::Result<Option<Child>>;

/// The system's curl: Windows' own (Windows 10 1803 on), not one a folder on the search path
/// might hold; macOS's; elsewhere the one on the path.
fn curl_program() -> PathBuf {
    if cfg!(windows) {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        PathBuf::from(root).join("System32").join("curl.exe")
    } else if cfg!(target_os = "macos") {
        PathBuf::from("/usr/bin/curl")
    } else {
        PathBuf::from("curl")
    }
}

/// curl asking GitHub's API for `app`'s latest release, its answer to `out`: quiet, failing on
/// an HTTP error, by HTTPS only, within `CURL_SECONDS`, and on Windows without a console of
/// its own (one would flash up over the host).
pub fn curl(app: &App, out: File) -> io::Result<Child> {
    let seconds = CURL_SECONDS.to_string();
    let most = MOST.to_string();
    let agent = app.agent();
    let latest = app.latest();
    let mut c = Command::new(curl_program());
    c.args([
        "--silent",
        "--fail",
        "--location",
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "--max-time",
        &seconds,
        "--max-filesize",
        &most,
        "--header",
        "Accept: application/vnd.github+json",
        "--user-agent",
        &agent,
        &latest,
    ])
    .stdin(Stdio::null())
    .stdout(out)
    .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c.spawn()
}

/// `url` opened in the browser: by Windows' shell, macOS's `open`, else `xdg-open`.
pub fn browse(url: &str) -> io::Result<Option<Child>> {
    #[cfg(windows)]
    {
        shell::open(url).map(|()| None)
    }
    #[cfg(not(windows))]
    {
        let program = if cfg!(target_os = "macos") {
            "/usr/bin/open"
        } else {
            "xdg-open"
        };
        Command::new(program)
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(Some)
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod shell {
    use std::io;
    use std::iter::once;
    use std::ptr::null;

    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    /// `url` opened by the shell, in the user's browser.
    pub fn open(url: &str) -> io::Result<()> {
        let wide = |s: &str| s.encode_utf16().chain(once(0)).collect::<Vec<u16>>();
        let (verb, file) = (wide("open"), wide(url));
        // SAFETY: both strings are NUL-terminated UTF-16 and outlive the call; no window,
        // parameters or directory are given (null).
        let r = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                verb.as_ptr(),
                file.as_ptr(),
                null(),
                null(),
                SW_SHOWNORMAL,
            )
        };
        // Above 32 it opened it; else an error's code.
        let code = r as isize;
        if code > 32 {
            Ok(())
        } else {
            Err(io::Error::other(format!("ShellExecuteW: {code}")))
        }
    }
}

/// curl at work, its answer going into `file`.
struct Check {
    curl: Child,
    file: PathBuf,
    began: Instant,
}

/// Where a check stands.
enum Poll {
    Waiting,
    Answered(Vec<u8>),
    Failed,
}

impl Check {
    /// `fetch` started for `app`, its answer into a file of the check's own.
    fn start(app: &App, fetch: Fetch) -> io::Result<Check> {
        static COUNT: AtomicU32 = AtomicU32::new(0);
        let stem = app.name.to_ascii_lowercase().replace(['-', ' '], "");
        let mut tries = 0;
        let (file, out) = loop {
            let file = std::env::temp_dir().join(format!(
                "{stem}-latest-release-{}-{}.json",
                std::process::id(),
                COUNT.fetch_add(1, Ordering::Relaxed)
            ));
            // Made afresh: never a file already there, nor where a link there points.
            match File::options().write(true).create_new(true).open(&file) {
                Ok(out) => break (file, out),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists && tries < 8 => tries += 1,
                Err(e) => return Err(e),
            }
        };
        match fetch(app, out) {
            Ok(curl) => Ok(Check {
                curl,
                file,
                began: Instant::now(),
            }),
            Err(e) => {
                let _ = std::fs::remove_file(&file);
                Err(e)
            }
        }
    }

    fn poll(&mut self) -> Poll {
        match self.curl.try_wait() {
            Ok(None) if self.began.elapsed() < PATIENCE => Poll::Waiting,
            // Too slow (ended as the check is dropped), or failed.
            Ok(None) => Poll::Failed,
            Ok(Some(status)) if !status.success() => Poll::Failed,
            // Finished; or gone with its status unknown, in a host that reaps its children
            // itself (one that ignores SIGCHLD): what it wrote is judged by itself
            // (`release`), and curl writes nothing on an HTTP error.
            Ok(Some(_)) | Err(_) => self.answer().map_or(Poll::Failed, Poll::Answered),
        }
    }

    fn answer(&self) -> Option<Vec<u8>> {
        let mut a = Vec::new();
        File::open(&self.file)
            .ok()?
            .take(MOST + 1)
            .read_to_end(&mut a)
            .ok()?;
        (a.len() as u64 <= MOST).then_some(a)
    }
}

impl Drop for Check {
    /// curl ended if it still runs, and its file removed.
    fn drop(&mut self) {
        if matches!(self.curl.try_wait(), Ok(None)) {
            let _ = self.curl.kill();
        }
        let _ = self.curl.wait();
        let _ = std::fs::remove_file(&self.file);
    }
}

/// Where the update check stands.
enum State {
    /// Not asked yet.
    Idle,
    Checking(Check),
    /// This is the latest release (or later).
    Latest,
    Newer(Release),
    /// GitHub could not be asked, or its answer not read.
    Failed,
}

/// The colour of the update check's text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tone {
    #[default]
    Dim,
    /// A newer release.
    News,
    /// Something went wrong.
    Trouble,
}

/// What the drawer shows of the check: its text, in its colour, and its button's label.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Scene {
    pub text: String,
    pub tone: Tone,
    pub button: String,
}

/// The update check as the editor runs it: where it stands, what the drawer shows of it and
/// what its button does. Dropped (the editor closed), it ends curl.
pub struct Update {
    app: App,
    state: State,
    /// Whether the browser opened the last page asked of it (none asked yet).
    opened: Option<bool>,
    /// The processes opening pages (`open`, `xdg-open`), reaped as they end, one that failed
    /// counting as no browser opened; never ended by the editor (one may be the browser
    /// itself).
    openers: Vec<Child>,
    fetch: Fetch,
    open: Open,
}

impl std::fmt::Debug for Update {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Update")
            .field("app", &self.app)
            .field("scene", &self.scene())
            .finish()
    }
}

impl Update {
    /// The check for `app`, through the system's curl and browser.
    pub fn new(app: App) -> Self {
        Update::with(app, curl, browse)
    }

    /// The check for `app` through `fetch` and `open` (the tests' stand-ins: [`fake`]).
    pub fn with(app: App, fetch: Fetch, open: Open) -> Self {
        Update {
            app,
            state: State::Idle,
            opened: None,
            openers: Vec::new(),
            fetch,
            open,
        }
    }

    /// A press on its button: the check begun, the release's download or the releases' page
    /// opened.
    pub fn press(&mut self) {
        match &self.state {
            State::Idle | State::Latest => {
                self.opened = None;
                self.state = match Check::start(&self.app, self.fetch) {
                    Ok(c) => State::Checking(c),
                    Err(_) => State::Failed,
                };
            }
            State::Checking(_) => {}
            State::Newer(r) => {
                let url = r.download.clone();
                self.browse(&url);
            }
            State::Failed => {
                let url = self.app.releases();
                self.browse(&url);
            }
        }
    }

    fn browse(&mut self, url: &str) {
        match (self.open)(url) {
            Ok(opener) => {
                self.openers.extend(opener);
                self.opened = Some(true);
            }
            Err(_) => self.opened = Some(false),
        }
    }

    /// Once a frame: curl's answer read once it has finished, the openers that ended reaped.
    pub fn tick(&mut self) {
        let mut failed = false;
        self.openers.retain_mut(|o| match o.try_wait() {
            Ok(None) => true,
            Ok(Some(status)) => {
                failed |= !status.success();
                false
            }
            Err(_) => false,
        });
        if failed {
            self.opened = Some(false);
        }
        let State::Checking(c) = &mut self.state else {
            return;
        };
        self.state = match c.poll() {
            Poll::Waiting => return,
            Poll::Answered(a) => match release(&self.app, &a, INSTALLER) {
                Some(r) if newer(&r.version, self.app.version) => State::Newer(r),
                Some(_) => State::Latest,
                None => State::Failed,
            },
            Poll::Failed => State::Failed,
        };
    }

    /// Whether a check is under way (the editor draws a frame each tick meanwhile).
    pub fn checking(&self) -> bool {
        matches!(self.state, State::Checking(_))
    }

    /// The file curl writes its answer into, while a check is under way.
    pub fn answer_file(&self) -> Option<PathBuf> {
        match &self.state {
            State::Checking(c) => Some(c.file.clone()),
            _ => None,
        }
    }

    /// What the drawer shows of it.
    pub fn scene(&self) -> Scene {
        let (name, version) = (self.app.name, self.app.version);
        let (text, tone, button) = match &self.state {
            State::Idle => (format!("{name} {version}"), Tone::Dim, "CHECK FOR UPDATES"),
            State::Checking(_) => (format!("{name} {version}"), Tone::Dim, "CHECKING…"),
            State::Latest => (
                format!("{version} IS UP TO DATE"),
                Tone::Dim,
                "CHECK FOR UPDATES",
            ),
            State::Newer(_) if self.opened == Some(true) => (
                "CLOSE THE DAW, THEN INSTALL IT".to_owned(),
                Tone::News,
                "DOWNLOAD",
            ),
            State::Newer(r) => (
                format!("{} IS OUT (THIS IS {version})", r.version),
                Tone::News,
                "DOWNLOAD",
            ),
            State::Failed => ("COULD NOT CHECK".to_owned(), Tone::Trouble, "RELEASES PAGE"),
        };
        let (text, tone) = if self.opened == Some(false) {
            ("NO BROWSER WOULD OPEN".to_owned(), Tone::Trouble)
        } else {
            (text, tone)
        };
        Scene {
            text,
            tone,
            button: button.to_owned(),
        }
    }

    /// Every scene the check can show for `app`, a long version's among them, CHECKING…'s
    /// too: for a plug-in to test that each fits its drawer whole.
    pub fn every_scene(app: App) -> Vec<Scene> {
        let mut u = Update::with(app, fake::no_curl, fake::browser);
        let release = Release {
            version: "10.10.10".into(),
            download: app.releases(),
        };
        let mut scenes = vec![u.scene()];
        scenes.push(Scene {
            button: "CHECKING…".into(),
            ..u.scene()
        });
        u.state = State::Latest;
        scenes.push(u.scene());
        u.state = State::Newer(release);
        scenes.push(u.scene());
        u.press();
        scenes.push(u.scene());
        u.state = State::Failed;
        scenes.push(u.scene());
        u.opened = Some(false);
        scenes.push(u.scene());
        let _ = fake::opened();
        scenes
    }
}

/// Stand-ins for curl and the browser, for the tests here and a plug-in's editor's.
pub mod fake {
    use std::cell::RefCell;

    use super::*;

    thread_local! {
        static OPENED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    /// No curl to start.
    pub fn no_curl(_app: &App, _out: File) -> io::Result<Child> {
        Err(io::Error::from(io::ErrorKind::NotFound))
    }

    /// A browser that opens every page (this thread's record of them: [`opened`]).
    pub fn browser(url: &str) -> io::Result<Option<Child>> {
        OPENED.with(|o| o.borrow_mut().push(url.to_owned()));
        Ok(None)
    }

    /// No browser that will open.
    pub fn no_browser(_url: &str) -> io::Result<Option<Child>> {
        Err(io::Error::from(io::ErrorKind::NotFound))
    }

    /// The pages this thread's [`browser`] opened since it was last asked.
    pub fn opened() -> Vec<String> {
        OPENED.with(|o| std::mem::take(&mut *o.borrow_mut()))
    }
}

#[cfg(test)]
mod tests {
    use super::fake::*;
    use super::*;

    /// The plug-in these tests check for: this kit's version, a repository of its own.
    const APP: App = App {
        name: "KIT-TEST",
        version: env!("CARGO_PKG_VERSION"),
        repository: "https://github.com/idlefoundry/plugin-kit-test",
    };
    const RELEASES: &str = "https://github.com/idlefoundry/plugin-kit-test/releases";

    /// GitHub's answer for a release tagged `tag`, cut to what is read, its assets those the
    /// CA-72 0.1.0's had.
    fn answer(tag: &str) -> Vec<u8> {
        let v = tag.trim_start_matches('v');
        let asset = |n: &str| {
            serde_json::json!({
                "name": format!("KIT-TEST-{v}-{n}"),
                "browser_download_url": format!("{RELEASES}/download/{tag}/KIT-TEST-{v}-{n}"),
                "state": "uploaded",
            })
        };
        let assets: Vec<_> = [
            "git-sources.tar.gz",
            "Linux-x86_64.tar.gz",
            "macOS.pkg",
            "Windows-setup.exe",
        ]
        .into_iter()
        .map(asset)
        .collect();
        serde_json::to_vec(&serde_json::json!({
            "html_url": format!("{RELEASES}/tag/{tag}"),
            "tag_name": tag,
            "name": format!("KIT-TEST {v}"),
            "draft": false,
            "prerelease": false,
            "assets": assets,
        }))
        .unwrap()
    }

    /// The addresses a repository gives, and what curl calls itself.
    #[test]
    fn the_addresses_are_the_repositorys() {
        assert_eq!(APP.releases(), RELEASES);
        assert_eq!(
            APP.latest(),
            "https://api.github.com/repos/idlefoundry/plugin-kit-test/releases/latest"
        );
        let slash = App {
            repository: "https://github.com/idlefoundry/plugin-kit-test/",
            ..APP
        };
        assert_eq!(
            (slash.releases(), slash.latest()),
            (APP.releases(), APP.latest())
        );
        assert_eq!(
            APP.agent(),
            format!("KIT-TEST/{}", env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn a_version_is_three_numbers() {
        assert_eq!(numbers("0.1.0"), Some((0, 1, 0)));
        assert_eq!(numbers("12.0.345"), Some((12, 0, 345)));
        for odd in [
            "",
            "0.1",
            "0.1.0.1",
            "0.1.x",
            "0.2.0-beta.1",
            "v0.1.0",
            "0.+1.0",
            "0..1",
        ] {
            assert_eq!(numbers(odd), None, "{odd}");
        }
        // Later than this build's version, whichever it is: the next patch, minor and major;
        // not this one, nor 0.1.0, the first release.
        let this = APP.version;
        let (major, minor, patch) = numbers(this).expect("this build's version");
        for later in [
            format!("{major}.{minor}.{}", patch + 1),
            format!("{major}.{}.0", minor + 1),
            format!("{}.0.0", major + 1),
        ] {
            assert!(newer(&later, this), "{later}");
        }
        assert!(newer("99.0.0", this));
        assert!(
            !newer(this, this)
                && !newer("0.1.0", this)
                && !newer("0.0.9", this)
                && !newer("nightly", this)
        );
    }

    #[test]
    fn the_installer_for_each_system_is_found() {
        let a = answer("v0.2.0");
        for (installer, name) in [
            ("Windows-setup.exe", "KIT-TEST-0.2.0-Windows-setup.exe"),
            ("macOS.pkg", "KIT-TEST-0.2.0-macOS.pkg"),
            ("Linux-x86_64.tar.gz", "KIT-TEST-0.2.0-Linux-x86_64.tar.gz"),
        ] {
            assert_eq!(
                release(&APP, &a, Some(installer)),
                Some(Release {
                    version: "0.2.0".into(),
                    download: format!("{RELEASES}/download/v0.2.0/{name}"),
                })
            );
        }
        // None for this system, or the release without one: its page.
        let page = format!("{RELEASES}/tag/v0.2.0");
        assert_eq!(release(&APP, &a, None).unwrap().download, page);
        assert_eq!(
            release(&APP, &a, Some("Linux-aarch64.tar.gz"))
                .unwrap()
                .download,
            page
        );
        // This system's is one of the three.
        if let Some(installer) = INSTALLER {
            assert!(
                release(&APP, &a, INSTALLER)
                    .unwrap()
                    .download
                    .ends_with(installer)
            );
        }
    }

    /// The browser goes only to the repository's releases, whatever an answer says.
    #[test]
    fn the_browser_is_sent_only_to_the_releases() {
        let mut v: serde_json::Value = serde_json::from_slice(&answer("v0.2.0")).unwrap();
        v["assets"][3]["browser_download_url"] =
            "https://example.com/KIT-TEST-0.2.0-Windows-setup.exe".into();
        let a = serde_json::to_vec(&v).unwrap();
        assert_eq!(
            release(&APP, &a, Some("Windows-setup.exe"))
                .unwrap()
                .download,
            format!("{RELEASES}/tag/v0.2.0")
        );
        v["html_url"] = "https://github.com/someone/else/releases/tag/v0.2.0".into();
        let a = serde_json::to_vec(&v).unwrap();
        assert_eq!(
            release(&APP, &a, Some("Windows-setup.exe"))
                .unwrap()
                .download,
            RELEASES
        );
        assert!(ours(RELEASES, RELEASES) && ours(RELEASES, &format!("{RELEASES}/tag/v0.2.0")));
        for not in [
            "https://github.com/idlefoundry/plugin-kit-test/releases.example.com/x",
            "https://github.com/idlefoundry/plugin-kit-test/releases/../../other/releases",
            "https://github.com/idlefoundry/plugin-kit-test/releases/tag/v0.2.0?x=\"y\"",
            "https://github.com/idlefoundry/plugin-kit-test/releases/tag/v 0.2.0",
            "http://github.com/idlefoundry/plugin-kit-test/releases",
            "file:///C:/Windows/System32/calc.exe",
        ] {
            assert!(!ours(RELEASES, not), "{not}");
        }
    }

    #[test]
    fn an_answer_without_a_version_is_none() {
        assert_eq!(release(&APP, b"", INSTALLER), None);
        assert_eq!(release(&APP, b"<html>rate limited</html>", INSTALLER), None);
        assert_eq!(
            release(&APP, br#"{"message":"Not Found"}"#, INSTALLER),
            None
        );
        assert_eq!(release(&APP, &answer("nightly"), INSTALLER), None);
        assert_eq!(release(&APP, &answer("v0.2"), INSTALLER), None);
        // A tag without its `v` is read too.
        assert_eq!(
            release(&APP, &answer("0.2.0"), INSTALLER).unwrap().version,
            "0.2.0"
        );
    }

    /// Every scene the check shows, for a plug-in's drawer to fit: the seven, each with a
    /// button, the long version's among them.
    #[test]
    fn every_scene_is_listed() {
        let scenes = Update::every_scene(APP);
        let texts: Vec<&str> = scenes.iter().map(|s| s.text.as_str()).collect();
        let this = APP.version;
        assert_eq!(
            texts,
            [
                format!("KIT-TEST {this}").as_str(),
                format!("KIT-TEST {this}").as_str(),
                format!("{this} IS UP TO DATE").as_str(),
                format!("10.10.10 IS OUT (THIS IS {this})").as_str(),
                "CLOSE THE DAW, THEN INSTALL IT",
                "COULD NOT CHECK",
                "NO BROWSER WOULD OPEN",
            ]
        );
        assert_eq!(scenes[1].button, "CHECKING…");
        assert!(scenes.iter().all(|s| !s.button.is_empty()));
        assert!(opened().is_empty(), "nothing left in the record");
    }

    /// A canned answer, by the release it names, for a stand-in for curl to print.
    fn canned(release: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "plugin-kit-update-tests-{}-{release}.json",
            std::process::id()
        ))
    }

    /// Something that prints `file`, as curl prints GitHub's answer.
    fn print(file: PathBuf, out: File) -> io::Result<Child> {
        let mut c = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.arg("/C").arg("type").arg(file);
            c
        } else {
            let mut c = Command::new("cat");
            c.arg(file);
            c
        };
        c.stdout(out).stderr(Stdio::null()).spawn()
    }

    /// GitHub naming 99.0.0, and this build's version, the latest.
    fn names_99(_app: &App, out: File) -> io::Result<Child> {
        print(canned("99"), out)
    }

    fn names_this(_app: &App, out: File) -> io::Result<Child> {
        print(canned("this"), out)
    }

    /// Something that fails, as curl does on an HTTP error.
    fn failing(_app: &App, out: File) -> io::Result<Child> {
        let mut c = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.args(["/C", "exit 22"]);
            c
        } else {
            let mut c = Command::new("sh");
            c.args(["-c", "exit 22"]);
            c
        };
        c.stdout(out).spawn()
    }

    /// Something that takes far longer than anyone waits.
    fn stuck(_app: &App, out: File) -> io::Result<Child> {
        let mut c = if cfg!(windows) {
            let mut c = Command::new("ping");
            c.args(["-n", "120", "127.0.0.1"]);
            c
        } else {
            let mut c = Command::new("sleep");
            c.arg("120");
            c
        };
        c.stdout(out).spawn()
    }

    /// The frames until the check is over (10 s at most).
    fn settle(u: &mut Update) {
        let t0 = Instant::now();
        while u.checking() {
            assert!(t0.elapsed() < Duration::from_secs(10), "still checking");
            u.tick();
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn file_of(u: &Update) -> PathBuf {
        u.answer_file().expect("checking")
    }

    /// A newer release: its version shown, DOWNLOAD opening its installer, and what to do
    /// with it; the answer's file gone once read.
    #[test]
    fn a_newer_release_offers_its_installer() {
        std::fs::write(canned("99"), answer("v99.0.0")).unwrap();
        let this = APP.version;
        let mut u = Update::with(APP, names_99, browser);
        let s = u.scene();
        assert_eq!(
            (s.text.as_str(), s.button.as_str()),
            (format!("KIT-TEST {this}").as_str(), "CHECK FOR UPDATES")
        );
        u.press();
        assert_eq!(u.scene().button, "CHECKING…");
        let file = file_of(&u);
        // Pressed again while it checks: nothing more.
        u.press();
        assert_eq!(file_of(&u), file);
        settle(&mut u);
        assert!(!file.exists(), "{file:?} left behind");
        let s = u.scene();
        assert_eq!(
            s,
            Scene {
                text: format!("99.0.0 IS OUT (THIS IS {this})"),
                tone: Tone::News,
                button: "DOWNLOAD".into(),
            }
        );
        u.press();
        let want = match INSTALLER {
            Some(i) => format!("{RELEASES}/download/v99.0.0/KIT-TEST-99.0.0-{i}"),
            None => format!("{RELEASES}/tag/v99.0.0"),
        };
        assert_eq!(opened(), vec![want]);
        assert_eq!(u.scene().text, "CLOSE THE DAW, THEN INSTALL IT");
        // A browser that will not open says so.
        u.open = no_browser;
        u.press();
        assert_eq!(
            (u.scene().text.as_str(), u.scene().tone),
            ("NO BROWSER WOULD OPEN", Tone::Trouble)
        );
        let _ = std::fs::remove_file(canned("99"));
    }

    /// This build's release the latest: up to date, and it may be asked again.
    #[test]
    fn the_latest_release_is_up_to_date() {
        let this = APP.version;
        std::fs::write(canned("this"), answer(&format!("v{this}"))).unwrap();
        let mut u = Update::with(APP, names_this, browser);
        u.press();
        settle(&mut u);
        assert_eq!(
            u.scene(),
            Scene {
                text: format!("{this} IS UP TO DATE"),
                tone: Tone::Dim,
                button: "CHECK FOR UPDATES".into(),
            }
        );
        // Asked again: checked again.
        u.press();
        assert_eq!(u.scene().button, "CHECKING…");
        settle(&mut u);
        assert_eq!(u.scene().text, format!("{this} IS UP TO DATE"));
        assert!(opened().is_empty());
        let _ = std::fs::remove_file(canned("this"));
    }

    /// curl missing, or failing: the releases' page offered instead.
    #[test]
    fn a_failed_check_offers_the_releases_page() {
        for fetch in [no_curl as Fetch, failing] {
            let mut u = Update::with(APP, fetch, browser);
            u.press();
            settle(&mut u);
            assert_eq!(
                u.scene(),
                Scene {
                    text: "COULD NOT CHECK".into(),
                    tone: Tone::Trouble,
                    button: "RELEASES PAGE".into(),
                }
            );
            u.press();
            assert_eq!(opened(), vec![RELEASES.to_owned()]);
        }
    }

    /// The editor closed while curl works: curl ended at once (not waited for), its file
    /// gone.
    #[test]
    fn closing_the_editor_ends_curl() {
        let mut u = Update::with(APP, stuck, browser);
        u.press();
        let file = file_of(&u);
        std::thread::sleep(Duration::from_millis(200));
        u.tick();
        assert!(u.checking());
        let t0 = Instant::now();
        drop(u);
        assert!(t0.elapsed() < Duration::from_secs(5), "{:?}", t0.elapsed());
        assert!(!file.exists(), "{file:?} left behind");
    }

    /// An opener that exits with `code`, as `xdg-open` does.
    fn exiting(code: u8) -> io::Result<Option<Child>> {
        let mut c = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.arg("/C").arg(format!("exit {code}"));
            c
        } else {
            let mut c = Command::new("sh");
            c.arg("-c").arg(format!("exit {code}"));
            c
        };
        c.spawn().map(Some)
    }

    fn opener_fails(_url: &str) -> io::Result<Option<Child>> {
        exiting(4)
    }

    fn opener_succeeds(_url: &str) -> io::Result<Option<Child>> {
        exiting(0)
    }

    /// An opener that fails once started (`xdg-open` with no browser, or no display): no
    /// browser opened, said once it has ended; one that ends well changes nothing.
    #[test]
    fn an_opener_that_fails_opens_no_browser() {
        for (open, text) in [
            (opener_succeeds as Open, "COULD NOT CHECK"),
            (opener_fails, "NO BROWSER WOULD OPEN"),
        ] {
            let mut u = Update::with(APP, no_curl, open);
            // The check fails at once; RELEASES PAGE.
            u.press();
            u.press();
            let t0 = Instant::now();
            while !u.openers.is_empty() {
                assert!(t0.elapsed() < Duration::from_secs(10), "still opening");
                u.tick();
                std::thread::sleep(Duration::from_millis(5));
            }
            assert_eq!(u.scene().text, text);
        }
    }

    /// A plug-in with releases: the CA-72, as a version before its first, so that its latest
    /// is newer.
    const PUBLISHED: App = App {
        name: "CA-72",
        version: "0.0.1",
        repository: "https://github.com/idlefoundry/ca-72",
    };

    /// GitHub asked by the system's curl (`cargo test -p plugin-kit-update -- --ignored
    /// github_names_its_latest_release`; the network's).
    #[test]
    #[ignore = "asks GitHub"]
    fn github_names_its_latest_release() {
        let mut u = Update::with(PUBLISHED, curl, browser);
        u.press();
        let t0 = Instant::now();
        while u.checking() {
            assert!(t0.elapsed() < PATIENCE + Duration::from_secs(1));
            u.tick();
            std::thread::sleep(Duration::from_millis(20));
        }
        eprintln!("{:?}", u.scene());
        assert!(matches!(u.state, State::Newer(_)), "{u:?}");
    }

    /// `url` opened in this system's browser as DOWNLOAD and RELEASES PAGE open it: the
    /// opener (macOS's `open`, `xdg-open`) ended well, or runs on (the browser itself).
    fn opens(url: &str) {
        let opener = browse(url).expect("a browser opened");
        if let Some(mut o) = opener {
            let t0 = Instant::now();
            while t0.elapsed() < Duration::from_secs(15) {
                if let Some(status) = o.try_wait().unwrap() {
                    assert!(status.success(), "{status}");
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }

    /// The releases' page opened in this system's browser (`cargo test -p plugin-kit-update --
    /// --ignored the_browser_opens_the_releases_page`).
    #[test]
    #[ignore = "opens the browser"]
    fn the_browser_opens_the_releases_page() {
        opens(&PUBLISHED.releases());
    }

    /// GitHub asked by the system's curl, and the latest release's installer for this system
    /// downloaded by the browser, as DOWNLOAD does a newer one's (`cargo test -p
    /// plugin-kit-update -- --ignored the_browser_downloads_this_systems_installer`).
    #[test]
    #[ignore = "asks GitHub and downloads an installer"]
    fn the_browser_downloads_this_systems_installer() {
        let mut c = Check::start(&PUBLISHED, curl).unwrap();
        let t0 = Instant::now();
        let answer = loop {
            match c.poll() {
                Poll::Waiting => {
                    assert!(t0.elapsed() < PATIENCE + Duration::from_secs(1));
                    std::thread::sleep(Duration::from_millis(20));
                }
                Poll::Answered(a) => break a,
                Poll::Failed => panic!("no answer from GitHub"),
            }
        };
        let r = release(&PUBLISHED, &answer, INSTALLER).expect("a release");
        eprintln!("{r:?}");
        if let Some(i) = INSTALLER {
            assert!(r.download.ends_with(&format!("{}-{i}", r.version)), "{r:?}");
        }
        opens(&r.download);
    }
}
