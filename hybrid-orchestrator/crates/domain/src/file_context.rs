//! Pure, framework-agnostic helpers for the attach/upload + repository-context
//! feature (FEAT-003, architecture.md Section 8.1 chat surface).
//!
//! The `tauri-app` crate builds ONLY in CI (it depends on the external `tauri`
//! crate, unreachable in the offline sandbox), so any logic living there cannot
//! be exercised by `cargo test` offline. To keep the failure-prone bits
//! (extension-based MIME guessing, the directory skip-list, binary-extension
//! filtering, and per-file / total byte-cap enforcement) HOST-VERIFIABLE, they
//! live here in the leaf `domain` crate, which builds and tests standalone. The
//! Tauri commands (`read_text_file`, `read_file_base64`, `list_repo_files`) are
//! thin `std::fs` wrappers over these helpers.
//!
//! None of these functions touch the filesystem: they operate on paths, names,
//! and byte counts so they are trivially unit-testable without temp files.

/// Maximum bytes accepted for a single attached TEXT file (256 KiB). A text
/// attachment is meant to be pasted-scale context, not a whole dataset; the cap
/// guards the assembled prompt (and the core) against unbounded input.
pub const MAX_ATTACH_BYTES: usize = 256 * 1024;

/// Maximum bytes accepted for a single attached IMAGE file (4 MiB). Larger than
/// the text cap because images are inherently bigger, but still bounded so a
/// base64 payload cannot blow up the IPC message.
pub const MAX_IMAGE_BYTES: usize = 4 * 1024 * 1024;

/// Maximum number of file entries returned by a repository listing. A picked
/// folder can contain thousands of files; the UI only needs a bounded set to
/// choose from, so the walk stops after this many entries and flags the result
/// as truncated.
pub const MAX_REPO_ENTRIES: usize = 500;

/// Directory names skipped wholesale when listing a repository: version-control
/// metadata and dependency / build-output trees that are noise for context
/// selection and can be enormous.
pub const SKIPPED_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    ".venv",
    "venv",
    "__pycache__",
    ".next",
    ".cache",
];

/// File extensions treated as binary and skipped from a repository listing (and
/// rejected by the text-read command). Lowercase, without the leading dot.
pub const BINARY_EXTENSIONS: &[&str] = &[
    // images
    "png",
    "jpg",
    "jpeg",
    "gif",
    "bmp",
    "ico",
    "webp",
    "tiff",
    "svg",
    // archives / compiled
    "zip",
    "gz",
    "tar",
    "rar",
    "7z",
    "exe",
    "dll",
    "so",
    "dylib",
    "o",
    "a",
    "class",
    "jar",
    "wasm",
    // media
    "mp3",
    "mp4",
    "wav",
    "avi",
    "mov",
    "mkv",
    "flac",
    "ogg",
    // documents / fonts / db
    "pdf",
    "doc",
    "docx",
    "xls",
    "xlsx",
    "ppt",
    "pptx",
    "ttf",
    "otf",
    "woff",
    "woff2",
    "db",
    "sqlite",
    "sqlite3",
    // model weights
    "gguf",
    "bin",
    "safetensors",
    "onnx",
    "pt",
    "pth",
];

/// Extract the final path component (the file or directory name) from a path.
///
/// Works with both `/` and `\` separators so a Windows-style path picked on
/// Windows still yields a clean name. Returns the whole input when there is no
/// separator, and an empty string only for an empty input.
pub fn file_name(path: &str) -> String {
    let trimmed = path.trim_end_matches(['/', '\\']);
    match trimmed.rsplit(['/', '\\']).next() {
        Some(name) if !name.is_empty() => name.to_string(),
        _ => trimmed.to_string(),
    }
}

/// The lowercase extension of a path (without the leading dot), or `None` when
/// there is none. A leading-dot name with no other dot (e.g. `.gitignore`) has
/// no extension.
pub fn extension_of(path: &str) -> Option<String> {
    let name = file_name(path);
    let dot = name.rfind('.')?;
    // A dot at index 0 is a dotfile (`.gitignore`), not an extension separator.
    if dot == 0 {
        return None;
    }
    let ext = &name[dot + 1..];
    if ext.is_empty() {
        None
    } else {
        Some(ext.to_ascii_lowercase())
    }
}

/// Whether a directory name should be skipped wholesale during a repo walk.
pub fn is_skipped_dir(name: &str) -> bool {
    SKIPPED_DIRS.contains(&name)
}

/// Whether a path's extension marks it as a binary file to skip / reject as
/// text. Extension-less files are treated as NON-binary (many text files -
/// `Makefile`, `Dockerfile`, `LICENSE` - have no extension).
pub fn has_binary_extension(path: &str) -> bool {
    match extension_of(path) {
        Some(ext) => BINARY_EXTENSIONS.contains(&ext.as_str()),
        None => false,
    }
}

/// Guess a MIME type from a path's extension for the base64 image command.
/// Falls back to `application/octet-stream` for anything unrecognized so the
/// return value is always a well-formed type string.
pub fn guess_mime_type(path: &str) -> &'static str {
    match extension_of(path).as_deref() {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("bmp") => "image/bmp",
        Some("svg") => "image/svg+xml",
        Some("tiff") => "image/tiff",
        Some("ico") => "image/x-icon",
        Some("txt") | Some("md") | Some("markdown") => "text/plain",
        Some("json") => "application/json",
        _ => "application/octet-stream",
    }
}

/// Whether a byte length is within the per-file text-attachment cap.
pub fn within_text_cap(byte_len: usize) -> bool {
    byte_len <= MAX_ATTACH_BYTES
}

/// Whether a byte length is within the per-file image cap.
pub fn within_image_cap(byte_len: usize) -> bool {
    byte_len <= MAX_IMAGE_BYTES
}

/// The standard base64 alphabet (RFC 4648), used by [`encode_base64`].
const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encode a byte slice as standard base64 (RFC 4648, with `=` padding).
///
/// Hand-rolled rather than pulling a new crate dependency: the `tauri-app`
/// crate is `[workspace] exclude`d and every dep must be pinned as a concrete
/// literal, and this keeps the (small, well-defined) encoding host-verifiable in
/// the leaf `domain` crate instead of only in the CI-only shell.
pub fn encode_base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as usize;
        let b1 = if chunk.len() > 1 {
            chunk[1] as usize
        } else {
            0
        };
        let b2 = if chunk.len() > 2 {
            chunk[2] as usize
        } else {
            0
        };
        out.push(BASE64_ALPHABET[b0 >> 2] as char);
        out.push(BASE64_ALPHABET[((b0 & 0b11) << 4) | (b1 >> 4)] as char);
        if chunk.len() > 1 {
            out.push(BASE64_ALPHABET[((b1 & 0b1111) << 2) | (b2 >> 6)] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(BASE64_ALPHABET[b2 & 0b111111] as char);
        } else {
            out.push('=');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_name_handles_both_separators_and_edges() {
        assert_eq!(file_name("/home/user/notes.txt"), "notes.txt");
        assert_eq!(file_name("C:\\Users\\me\\a.md"), "a.md");
        assert_eq!(file_name("bare.txt"), "bare.txt");
        assert_eq!(file_name("/trailing/dir/"), "dir");
        assert_eq!(file_name(""), "");
    }

    #[test]
    fn extension_of_is_lowercased_and_skips_dotfiles() {
        assert_eq!(extension_of("a.TXT").as_deref(), Some("txt"));
        assert_eq!(extension_of("archive.tar.GZ").as_deref(), Some("gz"));
        assert_eq!(extension_of(".gitignore"), None);
        assert_eq!(extension_of("Makefile"), None);
        assert_eq!(extension_of("trailing."), None);
    }

    #[test]
    fn skipped_dirs_cover_common_noise() {
        assert!(is_skipped_dir(".git"));
        assert!(is_skipped_dir("node_modules"));
        assert!(is_skipped_dir("target"));
        assert!(!is_skipped_dir("src"));
        assert!(!is_skipped_dir("lib"));
    }

    #[test]
    fn binary_extension_detection() {
        assert!(has_binary_extension("logo.png"));
        assert!(has_binary_extension("weights.GGUF"));
        assert!(has_binary_extension("bundle.zip"));
        assert!(!has_binary_extension("main.rs"));
        assert!(!has_binary_extension("README.md"));
        // extension-less text files are not binary
        assert!(!has_binary_extension("Dockerfile"));
        assert!(!has_binary_extension("LICENSE"));
    }

    #[test]
    fn mime_guess_covers_images_and_falls_back() {
        assert_eq!(guess_mime_type("a.png"), "image/png");
        assert_eq!(guess_mime_type("a.JPG"), "image/jpeg");
        assert_eq!(guess_mime_type("a.jpeg"), "image/jpeg");
        assert_eq!(guess_mime_type("a.gif"), "image/gif");
        assert_eq!(guess_mime_type("a.webp"), "image/webp");
        assert_eq!(guess_mime_type("a.unknownext"), "application/octet-stream");
        assert_eq!(guess_mime_type("noext"), "application/octet-stream");
    }

    #[test]
    fn caps_enforce_bounds() {
        assert!(within_text_cap(0));
        assert!(within_text_cap(MAX_ATTACH_BYTES));
        assert!(!within_text_cap(MAX_ATTACH_BYTES + 1));
        assert!(within_image_cap(MAX_IMAGE_BYTES));
        assert!(!within_image_cap(MAX_IMAGE_BYTES + 1));
    }

    #[test]
    fn base64_matches_known_vectors() {
        // RFC 4648 test vectors.
        assert_eq!(encode_base64(b""), "");
        assert_eq!(encode_base64(b"f"), "Zg==");
        assert_eq!(encode_base64(b"fo"), "Zm8=");
        assert_eq!(encode_base64(b"foo"), "Zm9v");
        assert_eq!(encode_base64(b"foob"), "Zm9vYg==");
        assert_eq!(encode_base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(encode_base64(b"foobar"), "Zm9vYmFy");
        // A byte with the high bit set exercises the full alphabet.
        assert_eq!(encode_base64(&[0xff, 0xff, 0xff]), "////");
    }
}
