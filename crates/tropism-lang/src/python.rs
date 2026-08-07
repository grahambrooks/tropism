//! Python provider.
//!
//! Three things here are unlike the first four languages.
//!
//! **The import name is not the package name, and no rule recovers it.** `import
//! yaml` comes from `PyYAML`, `import cv2` from `opencv-python`, `import sklearn`
//! from `scikit-learn`. The mapping lives in each distribution's installed
//! metadata, which only exists after an install — so a hermetic tool has a curated
//! table and an honest `Unresolved` for everything else. This is the import→package
//! problem at its worst; see `design/03-language-providers.md`.
//!
//! **A module is a file, not a directory.** `pkg/a.py` and `pkg/b.py` are separate
//! modules that can import each other, and a cycle between two files in one package
//! is a real defect Python reports only at runtime, as a partially-initialised
//! module. Directory granularity would hide exactly the cycles worth finding.
//!
//! **`src/` is a layout convention, not a package.** `src/app/main.py` is the module
//! `app.main`; treating `src` as a package segment would make every import in a
//! src-layout project unresolvable.

use camino::Utf8Path;
use tropism_core::graph::ModuleId;
use tropism_core::model::{DeclaredDep, DepKind, Language, Manifest, Provenance, ResolvedDep};
use tropism_core::provider::{Import, ImportTarget, LanguageProvider, ProjectContext, VersionOps};

pub struct PythonProvider;

/// Top-level modules that ship with CPython.
///
/// Unlike Go's, this list cannot be replaced by a structural rule: `requests` and
/// `random` are the same shape. Being wrong in the safe direction matters — a
/// missing entry produces a false missing-dependency finding, so the list errs
/// towards inclusion and covers the deprecated names still seen in real code.
const STDLIB: &[&str] = &[
    "__future__",
    "__main__",
    "_thread",
    "abc",
    "argparse",
    "array",
    "ast",
    "asyncio",
    "atexit",
    "base64",
    "bdb",
    "binascii",
    "bisect",
    "builtins",
    "bz2",
    "calendar",
    "cgi",
    "cmath",
    "cmd",
    "code",
    "codecs",
    "collections",
    "colorsys",
    "compileall",
    "concurrent",
    "configparser",
    "contextlib",
    "contextvars",
    "copy",
    "copyreg",
    "csv",
    "ctypes",
    "curses",
    "dataclasses",
    "datetime",
    "dbm",
    "decimal",
    "difflib",
    "dis",
    "doctest",
    "email",
    "encodings",
    "enum",
    "errno",
    "faulthandler",
    "fcntl",
    "filecmp",
    "fileinput",
    "fnmatch",
    "fractions",
    "ftplib",
    "functools",
    "gc",
    "getopt",
    "getpass",
    "gettext",
    "glob",
    "graphlib",
    "gzip",
    "hashlib",
    "heapq",
    "hmac",
    "html",
    "http",
    "imaplib",
    "importlib",
    "inspect",
    "io",
    "ipaddress",
    "itertools",
    "json",
    "keyword",
    "linecache",
    "locale",
    "logging",
    "lzma",
    "mailbox",
    "marshal",
    "math",
    "mimetypes",
    "mmap",
    "multiprocessing",
    "netrc",
    "numbers",
    "operator",
    "os",
    "pathlib",
    "pdb",
    "pickle",
    "pickletools",
    "pkgutil",
    "platform",
    "plistlib",
    "poplib",
    "posixpath",
    "pprint",
    "profile",
    "pstats",
    "pty",
    "pwd",
    "py_compile",
    "pyclbr",
    "pydoc",
    "queue",
    "quopri",
    "random",
    "re",
    "readline",
    "reprlib",
    "resource",
    "rlcompleter",
    "runpy",
    "sched",
    "secrets",
    "select",
    "selectors",
    "shelve",
    "shlex",
    "shutil",
    "signal",
    "site",
    "smtplib",
    "socket",
    "socketserver",
    "sqlite3",
    "ssl",
    "stat",
    "statistics",
    "string",
    "stringprep",
    "struct",
    "subprocess",
    "symtable",
    "sys",
    "sysconfig",
    "syslog",
    "tarfile",
    "tempfile",
    "termios",
    "textwrap",
    "threading",
    "time",
    "timeit",
    "tkinter",
    "token",
    "tokenize",
    "tomllib",
    "trace",
    "traceback",
    "tracemalloc",
    "tty",
    "types",
    "typing",
    "unicodedata",
    "unittest",
    "urllib",
    "uuid",
    "venv",
    "warnings",
    "wave",
    "weakref",
    "webbrowser",
    "wsgiref",
    "xml",
    "xmlrpc",
    "zipapp",
    "zipfile",
    "zipimport",
    "zlib",
    "zoneinfo",
];

/// Import names whose distribution is not derivable from the name.
///
/// The residue after normalization, which handles the ordinary case
/// (`import ruamel.yaml` from `ruamel.yaml`, `import attrs` from `attrs`). Kept
/// small and curated on purpose: a wrong entry becomes a confident false finding,
/// so an unfamiliar name is left to the caller's declared list rather than guessed
/// at. Every entry here is a top-30 PyPI distribution whose import name differs.
const IMPORT_TO_DISTRIBUTION: &[(&str, &str)] = &[
    ("attr", "attrs"),
    ("bs4", "beautifulsoup4"),
    ("cv2", "opencv-python"),
    ("dateutil", "python-dateutil"),
    ("dns", "dnspython"),
    ("dotenv", "python-dotenv"),
    ("fitz", "pymupdf"),
    ("git", "gitpython"),
    ("jose", "python-jose"),
    ("jwt", "pyjwt"),
    ("magic", "python-magic"),
    ("mpl_toolkits", "matplotlib"),
    ("nacl", "pynacl"),
    ("pkg_resources", "setuptools"),
    ("psycopg2", "psycopg2-binary"),
    ("pylab", "matplotlib"),
    ("serial", "pyserial"),
    ("setuptools", "setuptools"),
    ("skimage", "scikit-image"),
    ("sklearn", "scikit-learn"),
    ("slugify", "python-slugify"),
    ("sqlalchemy", "sqlalchemy"),
    ("usb", "pyusb"),
    ("win32com", "pywin32"),
    ("yaml", "pyyaml"),
    ("zoneinfo", "backports-zoneinfo"),
    ("OpenSSL", "pyopenssl"),
    ("PIL", "pillow"),
];

struct Pep440Ops;

impl VersionOps for Pep440Ops {
    /// Compares the release segment, which is what conflict findings are about.
    ///
    /// A full PEP 440 implementation would also order epochs, pre-releases, and
    /// local versions. Those appear in a lockfile rarely and never change which
    /// *pair* of versions conflicts, so the missing precision costs nothing here —
    /// and `None` on anything unparseable keeps a wrong ordering out of a finding.
    fn compare(&self, a: &str, b: &str) -> Option<std::cmp::Ordering> {
        let left = release_segment(a)?;
        let right = release_segment(b)?;
        Some(left.cmp(&right))
    }

    /// Not implemented: no check needs it, and PEP 440 specifiers include `~=` and
    /// `===` whose semantics are worth getting right rather than approximating.
    fn satisfies(&self, _version: &str, _requirement: &str) -> Option<bool> {
        None
    }
}

/// `1.2.3.post1` → `[1, 2, 3]`. `None` when the version does not start with a
/// numeric release, which is how a comparison declines rather than guesses.
fn release_segment(version: &str) -> Option<Vec<u64>> {
    let trimmed = version.trim().trim_start_matches('v');
    let release = trimmed
        .split(['-', '+', 'a', 'b', 'r', 'c', 'd', 'p'])
        .next()
        .unwrap_or(trimmed);
    let parts: Vec<u64> = release
        .split('.')
        .filter(|part| !part.is_empty())
        .map(|part| part.parse().ok())
        .collect::<Option<Vec<u64>>>()?;
    (!parts.is_empty()).then_some(parts)
}

impl LanguageProvider for PythonProvider {
    fn language(&self) -> Language {
        Language::Python
    }

    /// Sorted order decides which is parsed when a directory has both, and
    /// `pyproject.toml` sorts first — which is also the one to prefer, since it
    /// names the distribution and separates dependency groups.
    fn manifest_names(&self) -> &'static [&'static str] {
        &["pyproject.toml", "requirements.txt"]
    }

    /// `poetry.lock` and `uv.lock` are genuinely resolved trees: exact versions and
    /// the edges between them. `requirements.txt` pinned with `==` is not listed
    /// here — it is a manifest that happens to be pinned, carries no edges, and
    /// treating it as a lockfile would report a flat list as a resolved graph.
    fn lockfile_names(&self) -> &'static [&'static str] {
        &["uv.lock", "poetry.lock"]
    }

    fn source_extensions(&self) -> &'static [&'static str] {
        &["py", "pyi"]
    }

    fn parse_manifest(&self, path: &Utf8Path, text: &str) -> anyhow::Result<Manifest> {
        if path.file_name() == Some("requirements.txt") {
            Ok(parse_requirements(path, text))
        } else {
            parse_pyproject(path, text)
        }
    }

    fn parse_lockfile(
        &self,
        path: &Utf8Path,
        text: &str,
    ) -> anyhow::Result<Option<Vec<ResolvedDep>>> {
        parse_python_lock(path, text)
    }

    fn extract_imports(&self, path: &Utf8Path, text: &str) -> anyhow::Result<Vec<Import>> {
        extract_python_imports(path, text)
    }

    /// A Python module is a file's dotted path, with two adjustments that are not
    /// cosmetic: `__init__.py` *is* its package rather than a module inside it, and
    /// a leading `src/` is a layout convention that never appears in an import.
    fn module_id_for_file(&self, path: &Utf8Path, _text: &str, default_id: &str) -> ModuleId {
        let name = dotted_name(default_id, path);

        // Test modules import the code under test by design; that is never a cycle.
        // `conftest.py` is pytest's own hook file and is never imported at all.
        let file = path.file_name().unwrap_or_default();
        let is_test = file.starts_with("test_")
            || file.ends_with("_test.py")
            || file == "conftest.py"
            || path
                .as_str()
                .split('/')
                .any(|segment| segment == "tests" || segment == "test");
        if is_test {
            ModuleId::external_test(name)
        } else {
            ModuleId::module(name)
        }
    }

    fn resolve_import(
        &self,
        import: &Import,
        from: &Utf8Path,
        ctx: &ProjectContext<'_>,
    ) -> ImportTarget {
        let raw = import.raw.as_str();
        if raw.is_empty() {
            return ImportTarget::Unresolved {
                reason: "empty import".to_owned(),
            };
        }

        if raw.starts_with('.') {
            return resolve_relative(raw, from, ctx);
        }

        // 1. A module this project defines. Checked before the standard library
        //    because Python resolves it that way too: a local `types.py` really does
        //    shadow `types`, and pretending otherwise hides the resulting import.
        if let Some(module) =
            longest_dotted_prefix(raw, ctx.known_modules.iter().map(String::as_str))
        {
            return ImportTarget::Internal(module);
        }

        let top = top_level(raw);

        if self.is_stdlib(raw) {
            return ImportTarget::Stdlib;
        }

        // 2. Translate the import name to a distribution name *before* comparing
        //    against the manifest. Doing it the other way round means `import yaml`
        //    misses a declared `PyYAML`, and the project then collects both a false
        //    missing-dep on `pyyaml` and a false unused-dep on `PyYAML`.
        let distribution = IMPORT_TO_DISTRIBUTION
            .iter()
            .find(|(import_name, _)| *import_name == top)
            .map(|(_, distribution)| *distribution);
        let normalized = normalize(distribution.unwrap_or(top));

        // 3. A declared distribution, matched under PEP 503 normalization so
        //    `Flask-SQLAlchemy`, `flask_sqlalchemy`, and `flask.sqlalchemy` are one
        //    name. Authoritative when it matches, and returned in the manifest's own
        //    spelling so the hygiene checks compare like with like.
        if let Some(dep) = ctx
            .declared
            .iter()
            .find(|dep| normalize(&dep.name) == normalized)
        {
            return ImportTarget::External(dep.name.clone());
        }
        if let Some(sibling) = ctx
            .sibling_packages
            .iter()
            .find(|name| normalize(name) == normalized)
        {
            return ImportTarget::External(sibling.clone());
        }

        // 4. Everything else is a package this project imports without declaring —
        //    which is the finding. The name is normalized because that is the form a
        //    reader would put in the manifest to fix it.
        ImportTarget::External(normalized)
    }

    /// An absolute import names the module directly, so the module inside the target
    /// project is the longest one it declares.
    fn resolve_cross_project(
        &self,
        import: &Import,
        target: &ProjectContext<'_>,
    ) -> Option<String> {
        longest_dotted_prefix(&import.raw, target.known_modules.iter().map(String::as_str))
    }

    fn is_stdlib(&self, module: &str) -> bool {
        STDLIB.contains(&top_level(module))
    }

    fn version_ops(&self) -> &dyn VersionOps {
        &Pep440Ops
    }
}

/// The first dotted segment: `os.path` → `os`.
fn top_level(module: &str) -> &str {
    module.split('.').next().unwrap_or(module)
}

/// PEP 503 name normalization: case-folded, with runs of `-`, `_`, and `.`
/// collapsed to a single `-`. `Flask_SQLAlchemy` and `flask-sqlalchemy` are the
/// same distribution, and comparing the raw strings misses it.
fn normalize(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut last_was_separator = false;
    for ch in name.trim().chars() {
        if matches!(ch, '-' | '_' | '.') {
            if !last_was_separator && !out.is_empty() {
                out.push('-');
            }
            last_was_separator = true;
        } else {
            out.extend(ch.to_lowercase());
            last_was_separator = false;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// Whether `prefix` is a dotted-segment prefix of `module`.
fn is_dotted_prefix(module: &str, prefix: &str) -> bool {
    module == prefix
        || (module.len() > prefix.len()
            && module.starts_with(prefix)
            && module.as_bytes()[prefix.len()] == b'.')
}

fn longest_dotted_prefix<'a>(
    module: &str,
    candidates: impl Iterator<Item = &'a str>,
) -> Option<String> {
    candidates
        // "." is the root package of a flat project and is a prefix of nothing.
        .filter(|candidate| *candidate != "." && is_dotted_prefix(module, candidate))
        .max_by_key(|candidate| candidate.len())
        .map(str::to_owned)
}

/// The dotted module name for a source file.
///
/// `default_id` is the directory relative to the project root, which is what the
/// pipeline computes for every language; the file stem completes it.
fn dotted_name(default_id: &str, path: &Utf8Path) -> String {
    let stem = path.file_stem().unwrap_or_default();
    let mut parts: Vec<&str> = default_id
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();

    // `src/` is a packaging layout, never part of an import path. Only stripped at
    // the front, since a package genuinely named `src` deeper in the tree is
    // importable as `src`.
    if parts.first() == Some(&"src") {
        parts.remove(0);
    }

    // `pkg/__init__.py` *is* `pkg`, not `pkg.__init__`.
    if stem != "__init__" {
        parts.push(stem);
    }

    if parts.is_empty() {
        ".".to_owned()
    } else {
        parts.join(".")
    }
}

/// The package a file's relative imports are resolved against.
fn containing_package(project_root: &Utf8Path, file: &Utf8Path) -> Vec<String> {
    let relative = file.strip_prefix(project_root).unwrap_or(file);
    let mut parts: Vec<String> = relative
        .parent()
        .map(|parent| {
            parent
                .as_str()
                .split('/')
                .filter(|part| !part.is_empty() && *part != ".")
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    if parts.first().map(String::as_str) == Some("src") {
        parts.remove(0);
    }
    // A package's `__init__.py` sits *at* the package, so `from . import x` inside
    // it refers to that package rather than to its parent.
    if relative.file_stem() == Some("__init__")
        && let Some(last) = relative.parent().and_then(Utf8Path::file_name)
        && parts.last().map(String::as_str) == Some(last)
    {
        // already correct: the parent directory is the package
    }
    parts
}

/// Resolves `from .models import Order` and `from ..db import session`.
///
/// The leading dots count *upwards* from the containing package, so one dot is the
/// current package and each further dot removes a segment. Getting this wrong in
/// either direction invents edges between unrelated packages, so an over-deep
/// import stays unresolved rather than clamping to the root.
fn resolve_relative(raw: &str, from: &Utf8Path, ctx: &ProjectContext<'_>) -> ImportTarget {
    let level = raw.chars().take_while(|ch| *ch == '.').count();
    let tail = raw.trim_start_matches('.');

    let mut base = containing_package(&ctx.project.root, from);
    if level > base.len() + 1 {
        return ImportTarget::Unresolved {
            reason: format!("`{raw}` reaches above the project root"),
        };
    }
    base.truncate(base.len() + 1 - level);

    let candidate = if tail.is_empty() {
        base.join(".")
    } else if base.is_empty() {
        tail.to_owned()
    } else {
        format!("{}.{tail}", base.join("."))
    };

    // `from . import helpers` names either the submodule `pkg.helpers` or a symbol
    // defined in `pkg/__init__.py`. Preferring the submodule when one exists is the
    // only way to get file-level edges right; falling back to the package is what
    // makes the symbol case correct rather than unresolved.
    if !candidate.is_empty() && ctx.known_modules.contains(&candidate) {
        return ImportTarget::Internal(candidate);
    }
    let package = base.join(".");
    if !package.is_empty() && ctx.known_modules.contains(&package) {
        return ImportTarget::Internal(package);
    }

    ImportTarget::Unresolved {
        reason: format!("`{raw}` names no module in this project"),
    }
}

// --- manifests ---------------------------------------------------------------

/// Parses `pyproject.toml`: PEP 621, PEP 735, and Poetry.
///
/// All three shapes appear in the wild, often in the same repository, and a project
/// using Poetry has no `[project.dependencies]` at all — reading only the standard
/// one would report a Poetry project as declaring nothing.
fn parse_pyproject(path: &Utf8Path, text: &str) -> anyhow::Result<Manifest> {
    let root: toml::Value = toml::from_str(text)?;
    let mut deps: Vec<DeclaredDep> = Vec::new();

    let project = root.get("project");
    let package_name = project
        .and_then(|project| project.get("name"))
        .and_then(toml::Value::as_str)
        .or_else(|| root.get("tool")?.get("poetry")?.get("name")?.as_str())
        .map(str::to_owned);

    // PEP 621: an array of PEP 508 requirement strings.
    if let Some(list) = project
        .and_then(|project| project.get("dependencies"))
        .and_then(toml::Value::as_array)
    {
        for entry in list.iter().filter_map(toml::Value::as_str) {
            push_requirement(&mut deps, path, text, entry, DepKind::Runtime);
        }
    }

    // PEP 621 extras. An extra is opt-in for consumers but its packages are still
    // imported by this code, so Optional rather than Dev.
    if let Some(groups) = project
        .and_then(|project| project.get("optional-dependencies"))
        .and_then(toml::Value::as_table)
    {
        for list in groups.values().filter_map(toml::Value::as_array) {
            for entry in list.iter().filter_map(toml::Value::as_str) {
                push_requirement(&mut deps, path, text, entry, DepKind::Optional);
            }
        }
    }

    // PEP 735 dependency groups: `[dependency-groups] dev = [...]`. Never installed
    // for consumers, so Dev.
    if let Some(groups) = root
        .get("dependency-groups")
        .and_then(toml::Value::as_table)
    {
        for list in groups.values().filter_map(toml::Value::as_array) {
            for entry in list.iter().filter_map(toml::Value::as_str) {
                push_requirement(&mut deps, path, text, entry, DepKind::Dev);
            }
        }
    }

    // Poetry, whose dependencies are a table keyed by name.
    if let Some(poetry) = root.get("tool").and_then(|tool| tool.get("poetry")) {
        collect_poetry(
            &mut deps,
            path,
            text,
            poetry.get("dependencies"),
            DepKind::Runtime,
        );
        collect_poetry(
            &mut deps,
            path,
            text,
            poetry.get("dev-dependencies"),
            DepKind::Dev,
        );
        if let Some(groups) = poetry.get("group").and_then(toml::Value::as_table) {
            for group in groups.values() {
                collect_poetry(
                    &mut deps,
                    path,
                    text,
                    group.get("dependencies"),
                    DepKind::Dev,
                );
            }
        }
    }

    deps.sort_by_key(|a| (normalize(&a.name), a.kind));
    deps.dedup_by(|a, b| normalize(&a.name) == normalize(&b.name));

    Ok(Manifest { deps, package_name })
}

fn collect_poetry(
    deps: &mut Vec<DeclaredDep>,
    path: &Utf8Path,
    text: &str,
    table: Option<&toml::Value>,
    kind: DepKind,
) {
    let Some(table) = table.and_then(toml::Value::as_table) else {
        return;
    };
    for (name, spec) in table {
        // Poetry states the interpreter requirement as a dependency named `python`.
        // It is not a distribution and has no import.
        if name == "python" {
            continue;
        }
        let requirement = match spec {
            toml::Value::String(version) => version.clone(),
            toml::Value::Table(entry) => entry
                .get("version")
                .and_then(toml::Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            _ => String::new(),
        };
        deps.push(DeclaredDep {
            name: name.clone(),
            requirement,
            kind,
            declared_at: Provenance::new(path, find_line(text, name)),
        });
    }
}

fn push_requirement(
    deps: &mut Vec<DeclaredDep>,
    path: &Utf8Path,
    text: &str,
    entry: &str,
    kind: DepKind,
) {
    let Some(name) = requirement_name(entry) else {
        return;
    };
    deps.push(DeclaredDep {
        requirement: entry
            .strip_prefix(&name)
            .map(|rest| rest.trim().to_owned())
            .unwrap_or_default(),
        declared_at: Provenance::new(path, find_line(text, &name)),
        name,
        kind,
    });
}

/// The distribution name from a PEP 508 requirement.
///
/// `uvicorn[standard]>=0.30 ; python_version < "3.12"` → `uvicorn`. Everything
/// after the name is a version specifier, an extras list, or an environment
/// marker, none of which change which distribution is meant.
fn requirement_name(entry: &str) -> Option<String> {
    let entry = entry.trim();
    // A URL requirement (`name @ git+https://…`) still leads with the name.
    let head = entry.split('@').next().unwrap_or(entry);
    let name: String = head
        .chars()
        .take_while(|ch| ch.is_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        .collect();
    (!name.is_empty()).then_some(name)
}

/// Parses `requirements.txt`.
///
/// Line-oriented and simple, with one thing worth care: `-r base.txt` includes
/// another file. tropism does not follow it — the included file is discovered on its
/// own if it is named `requirements.txt`, and silently inlining an arbitrary path
/// would make a finding's provenance point at the wrong file.
fn parse_requirements(path: &Utf8Path, text: &str) -> Manifest {
    let mut deps = Vec::new();

    for (index, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or(raw).trim();
        if line.is_empty() || line.starts_with('-') {
            continue;
        }
        let Some(name) = requirement_name(line) else {
            continue;
        };
        deps.push(DeclaredDep {
            requirement: line
                .strip_prefix(&name)
                .map(|rest| rest.trim().to_owned())
                .unwrap_or_default(),
            declared_at: Provenance::new(path, Some(index as u32 + 1)),
            name,
            kind: DepKind::Runtime,
        });
    }

    deps.sort_by_key(|a| normalize(&a.name));
    deps.dedup_by(|a, b| normalize(&a.name) == normalize(&b.name));

    // requirements.txt names no distribution of its own.
    Manifest {
        deps,
        package_name: None,
    }
}

fn find_line(text: &str, needle: &str) -> Option<u32> {
    text.lines()
        .position(|line| line.contains(needle))
        .map(|index| index as u32 + 1)
}

/// Parses `uv.lock` or `poetry.lock`.
///
/// Both are TOML arrays of `[[package]]` with `name` and `version`, and both carry
/// edges — which is what makes the resolved-tree checks answerable for Python at
/// all. They differ only in how a dependency is written: uv uses an array of
/// tables, Poetry a table keyed by name.
///
/// One thing about Python shapes this. **The environment is flat**: `pip` installs
/// exactly one version of each distribution, so a lockfile has no way to say
/// "this copy of `urllib3`" and both formats name an edge by distribution alone.
/// When the resolution forks — different versions for different Python versions or
/// platforms — the same name appears twice and the edge becomes genuinely
/// ambiguous. Such an edge is dropped rather than attached to an arbitrary copy,
/// which is why `diamond-dep` reports nothing for a forked package: in a flat
/// environment there is no second copy for the dependents to disagree over.
fn parse_python_lock(path: &Utf8Path, text: &str) -> anyhow::Result<Option<Vec<ResolvedDep>>> {
    let root: toml::Value = toml::from_str(text)?;
    let Some(packages) = root.get("package").and_then(toml::Value::as_array) else {
        // A lockfile with no packages is empty, not malformed.
        return Ok(None);
    };

    let mut resolved = Vec::new();
    for entry in packages {
        let Some(name) = entry.get("name").and_then(toml::Value::as_str) else {
            continue;
        };
        let version = entry
            .get("version")
            .and_then(toml::Value::as_str)
            .unwrap_or_default();

        let mut dependencies: Vec<String> = Vec::new();
        match entry.get("dependencies") {
            // uv: `dependencies = [{ name = "idna" }, …]`
            Some(toml::Value::Array(list)) => {
                for item in list {
                    if let Some(name) = item.get("name").and_then(toml::Value::as_str) {
                        dependencies.push(normalize(name));
                    } else if let Some(name) = item.as_str() {
                        dependencies.push(normalize(
                            requirement_name(name).unwrap_or_default().as_str(),
                        ));
                    }
                }
            }
            // Poetry: `[package.dependencies] idna = "^3"`
            Some(toml::Value::Table(table)) => {
                dependencies.extend(table.keys().map(|name| normalize(name)));
            }
            _ => {}
        }

        dependencies.sort();
        dependencies.dedup();
        resolved.push(ResolvedDep {
            // Keyed by copy, not by name: two entries sharing a name are two
            // versions, and collapsing them would hide the conflict the key exists
            // to expose.
            key: format!("{} {version}", normalize(name)),
            name: name.to_owned(),
            version: version.to_owned(),
            dependencies,
        });
    }

    if resolved.is_empty() {
        anyhow::bail!("{path}: no packages found");
    }

    // Edges are written as bare names; turn each into the key of the copy it must
    // mean. A name with several copies is ambiguous without evaluating environment
    // markers — a resolver step — so those edges are dropped instead of guessed.
    let unique: std::collections::BTreeMap<String, String> = resolved
        .iter()
        .fold(
            std::collections::BTreeMap::<String, Vec<String>>::new(),
            |mut acc, dep| {
                acc.entry(normalize(&dep.name))
                    .or_default()
                    .push(dep.key.clone());
                acc
            },
        )
        .into_iter()
        .filter_map(|(name, keys)| match keys.as_slice() {
            [only] => Some((name, only.clone())),
            _ => None,
        })
        .collect();

    for dep in &mut resolved {
        dep.dependencies = dep
            .dependencies
            .iter()
            .filter_map(|name| unique.get(name).cloned())
            .collect();
    }

    Ok(Some(resolved))
}

// --- import extraction -------------------------------------------------------

/// Extracts every import from one Python file.
///
/// Both statement forms collapse to the same shape: the dotted path of the module
/// being reached. `from . import helpers` has no module path of its own, so the
/// imported names are appended to the dots — that is what lets resolution tell the
/// submodule `pkg.helpers` from a symbol defined in `pkg/__init__.py`.
fn extract_python_imports(path: &Utf8Path, text: &str) -> anyhow::Result<Vec<Import>> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .map_err(|error| anyhow::anyhow!("loading the Python grammar failed: {error}"))?;

    let tree = parser
        .parse(text, None)
        .ok_or_else(|| anyhow::anyhow!("tree-sitter could not parse `{path}`"))?;

    let mut imports = Vec::new();
    collect_imports(tree.root_node(), text.as_bytes(), &mut imports);
    Ok(imports)
}

fn collect_imports(node: tree_sitter::Node<'_>, source: &[u8], out: &mut Vec<Import>) {
    let line = node.start_position().row as u32 + 1;

    match node.kind() {
        "import_statement" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                match child.kind() {
                    "dotted_name" => push_text(child, source, line, out),
                    // `import numpy as np` — the module is the `name` field.
                    "aliased_import" => {
                        if let Some(name) = child.child_by_field_name("name") {
                            push_text(name, source, line, out);
                        }
                    }
                    _ => {}
                }
            }
            return;
        }
        "import_from_statement" | "future_import_statement" => {
            let module = node.child_by_field_name("module_name");
            let module_text = module
                .and_then(|node| node.utf8_text(source).ok())
                .unwrap_or_default()
                .to_owned();

            // A bare `from . import a, b` names its targets in the imported list;
            // anything else already names the module.
            if module_text.chars().all(|ch| ch == '.') && !module_text.is_empty() {
                let mut cursor = node.walk();
                let mut appended = false;
                for child in node.named_children(&mut cursor) {
                    if module.is_some_and(|module| child.id() == module.id()) {
                        continue;
                    }
                    let name = match child.kind() {
                        "dotted_name" => child.utf8_text(source).ok(),
                        "aliased_import" => child
                            .child_by_field_name("name")
                            .and_then(|name| name.utf8_text(source).ok()),
                        _ => None,
                    };
                    if let Some(name) = name {
                        out.push(Import::statement(format!("{module_text}{name}"), line));
                        appended = true;
                    }
                }
                // `from . import *` names nothing; the package itself is the target.
                if !appended {
                    out.push(Import::statement(module_text, line));
                }
            } else if !module_text.is_empty() {
                out.push(Import::statement(module_text, line));
            }
            return;
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_imports(child, source, out);
    }
}

fn push_text(node: tree_sitter::Node<'_>, source: &[u8], line: u32, out: &mut Vec<Import>) {
    if let Ok(text) = node.utf8_text(source) {
        out.push(Import::statement(text.to_owned(), line));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8PathBuf;
    use std::collections::BTreeSet;
    use tropism_core::model::Project;

    fn paths(source: &str) -> Vec<String> {
        extract_python_imports(Utf8Path::new("test.py"), source)
            .unwrap()
            .into_iter()
            .map(|import| import.raw)
            .collect()
    }

    // --- normalization ----------------------------------------------------

    #[test]
    fn pep_503_folds_case_and_separators() {
        assert_eq!(normalize("Flask-SQLAlchemy"), "flask-sqlalchemy");
        assert_eq!(normalize("flask_sqlalchemy"), "flask-sqlalchemy");
        assert_eq!(normalize("ruamel.yaml"), "ruamel-yaml");
        assert_eq!(normalize("zope--interface"), "zope-interface");
    }

    // --- pyproject.toml ---------------------------------------------------

    #[test]
    fn parses_pep_621_dependencies() {
        let manifest = parse_pyproject(
            Utf8Path::new("pyproject.toml"),
            "[project]\nname = \"svc\"\ndependencies = [\"requests>=2.31\", \"httpx\"]\n",
        )
        .unwrap();
        assert_eq!(manifest.package_name.as_deref(), Some("svc"));
        let names: Vec<&str> = manifest.deps.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["httpx", "requests"]);
    }

    #[test]
    fn a_pep_508_requirement_keeps_only_its_name() {
        assert_eq!(
            requirement_name("uvicorn[standard]>=0.30 ; python_version < \"3.12\""),
            Some("uvicorn".to_owned())
        );
        assert_eq!(
            requirement_name("pkg @ git+https://example.com/pkg"),
            Some("pkg".to_owned())
        );
    }

    #[test]
    fn parses_poetry_dependencies_and_skips_the_interpreter() {
        let manifest = parse_pyproject(
            Utf8Path::new("pyproject.toml"),
            "[tool.poetry]\nname = \"svc\"\n\n[tool.poetry.dependencies]\npython = \"^3.12\"\nrequests = \"^2.31\"\n",
        )
        .unwrap();
        let names: Vec<&str> = manifest.deps.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["requests"], "python is not a distribution");
        assert_eq!(manifest.package_name.as_deref(), Some("svc"));
    }

    #[test]
    fn dependency_groups_are_dev_dependencies() {
        let manifest = parse_pyproject(
            Utf8Path::new("pyproject.toml"),
            "[project]\nname = \"svc\"\n\n[dependency-groups]\ndev = [\"pytest\"]\n",
        )
        .unwrap();
        assert_eq!(manifest.deps[0].kind, DepKind::Dev);
    }

    #[test]
    fn a_malformed_pyproject_is_an_error_not_an_empty_manifest() {
        assert!(parse_pyproject(Utf8Path::new("pyproject.toml"), "[project\n").is_err());
    }

    // --- requirements.txt -------------------------------------------------

    #[test]
    fn parses_requirements_with_comments_and_flags() {
        let manifest = parse_requirements(
            Utf8Path::new("requirements.txt"),
            "# comment\n-r base.txt\nrequests==2.31.0\nhttpx  # inline\n\n--index-url https://x\n",
        );
        let names: Vec<&str> = manifest.deps.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["httpx", "requests"]);
        assert_eq!(manifest.deps[1].requirement, "==2.31.0");
    }

    // --- lockfiles --------------------------------------------------------

    #[test]
    fn parses_a_uv_lock_with_edges() {
        let resolved = parse_python_lock(
            Utf8Path::new("uv.lock"),
            "[[package]]\nname = \"requests\"\nversion = \"2.31.0\"\ndependencies = [{ name = \"idna\" }]\n\n[[package]]\nname = \"idna\"\nversion = \"3.6\"\n",
        )
        .unwrap()
        .unwrap();
        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[0].dependencies, vec!["idna 3.6"]);
    }

    #[test]
    fn parses_a_poetry_lock_with_edges() {
        let resolved = parse_python_lock(
            Utf8Path::new("poetry.lock"),
            "[[package]]\nname = \"requests\"\nversion = \"2.31.0\"\n\n[package.dependencies]\nidna = \"^3\"\n\n[[package]]\nname = \"idna\"\nversion = \"3.6\"\n",
        )
        .unwrap()
        .unwrap();
        assert_eq!(resolved[0].dependencies, vec!["idna 3.6"]);
    }

    /// Two copies of one name are two versions, and each keeps its own key — the
    /// version-conflict check has nothing to group otherwise.
    #[test]
    fn a_forked_resolution_keeps_both_copies() {
        let resolved = parse_python_lock(
            Utf8Path::new("uv.lock"),
            "[[package]]\nname = \"urllib3\"\nversion = \"1.26.18\"\n\n[[package]]\nname = \"urllib3\"\nversion = \"2.2.1\"\n",
        )
        .unwrap()
        .unwrap();
        assert_eq!(resolved.len(), 2);
        assert_ne!(resolved[0].key, resolved[1].key);
    }

    /// An edge naming a forked package cannot say which copy it means without
    /// evaluating markers, so it is dropped rather than attached to an arbitrary
    /// one — a wrong edge would become a confident false diamond finding.
    #[test]
    fn an_ambiguous_edge_is_dropped_rather_than_guessed() {
        let resolved = parse_python_lock(
            Utf8Path::new("uv.lock"),
            "[[package]]\nname = \"requests\"\nversion = \"2.31.0\"\ndependencies = [{ name = \"urllib3\" }]\n\n[[package]]\nname = \"urllib3\"\nversion = \"1.26.18\"\n\n[[package]]\nname = \"urllib3\"\nversion = \"2.2.1\"\n",
        )
        .unwrap()
        .unwrap();
        assert!(resolved[0].dependencies.is_empty());
    }

    // --- import extraction ------------------------------------------------

    #[test]
    fn extracts_plain_and_aliased_imports() {
        assert_eq!(
            paths("import os\nimport numpy as np\nimport a.b.c\n"),
            vec!["os", "numpy", "a.b.c"]
        );
    }

    #[test]
    fn extracts_from_imports_as_the_module() {
        assert_eq!(
            paths("from django.db import models\nfrom os import path\n"),
            vec!["django.db", "os"]
        );
    }

    /// `from . import helpers` is the one form whose target is in the *names*, not
    /// the module path.
    #[test]
    fn a_bare_relative_import_names_its_targets() {
        assert_eq!(
            paths("from . import helpers, models\n"),
            vec![".helpers", ".models"]
        );
        assert_eq!(paths("from .models import Order\n"), vec![".models"]);
        assert_eq!(paths("from ..db import session\n"), vec!["..db"]);
    }

    #[test]
    fn multiple_names_on_one_import_are_separate_modules() {
        assert_eq!(paths("import os, sys\n"), vec!["os", "sys"]);
    }

    /// The reason for a grammar instead of a regex.
    #[test]
    fn ignores_imports_in_strings_and_comments() {
        let source = concat!(
            "import os\n",
            "# import fake_commented\n",
            "doc = \"\"\"\nimport fake_docstring\n\"\"\"\n",
            "s = 'import fake_quoted'\n",
        );
        assert_eq!(paths(source), vec!["os"]);
    }

    #[test]
    fn conditional_and_nested_imports_are_found() {
        let source = "def f():\n    import json\n\nif True:\n    import csv\n";
        assert_eq!(paths(source), vec!["json", "csv"]);
    }

    #[test]
    fn reports_accurate_line_numbers() {
        let imports =
            extract_python_imports(Utf8Path::new("t.py"), "import os\n\nfrom sys import argv\n")
                .unwrap();
        assert_eq!(imports[0].line, 1);
        assert_eq!(imports[1].line, 3);
    }

    // --- module identity --------------------------------------------------

    fn module_of(file: &str, default_id: &str) -> ModuleId {
        PythonProvider.module_id_for_file(Utf8Path::new(file), "", default_id)
    }

    #[test]
    fn a_module_is_the_dotted_path_of_its_file() {
        assert_eq!(
            module_of("app/api/routes.py", "app/api").name,
            "app.api.routes"
        );
    }

    #[test]
    fn an_init_file_is_its_package() {
        assert_eq!(module_of("app/api/__init__.py", "app/api").name, "app.api");
    }

    /// `src/` is a layout convention and never appears in an import path.
    #[test]
    fn a_src_layout_prefix_is_not_part_of_the_module() {
        assert_eq!(module_of("src/app/main.py", "src/app").name, "app.main");
    }

    #[test]
    fn a_root_level_module_keeps_its_own_name() {
        assert_eq!(module_of("main.py", ".").name, "main");
    }

    /// Test modules import the code under test by design.
    #[test]
    fn test_modules_are_external_test_modules() {
        assert!(module_of("tests/test_api.py", "tests").kind.is_test());
        assert!(module_of("app/test_api.py", "app").kind.is_test());
        assert!(module_of("conftest.py", ".").kind.is_test());
        assert!(!module_of("app/api.py", "app").kind.is_test());
    }

    // --- resolution -------------------------------------------------------

    fn resolve(file: &str, raw: &str, declared: &[&str], modules: &[&str]) -> ImportTarget {
        let project = Project {
            root: Utf8PathBuf::from("svc"),
            language: Language::Python,
            manifests: vec![],
            lockfile: None,
        };
        let deps: Vec<DeclaredDep> = declared
            .iter()
            .map(|name| DeclaredDep {
                name: (*name).to_owned(),
                requirement: String::new(),
                kind: DepKind::Runtime,
                declared_at: Provenance::new("pyproject.toml", Some(1)),
            })
            .collect();
        let known: BTreeSet<String> = modules.iter().map(|m| (*m).to_owned()).collect();
        let ctx = ProjectContext {
            project: &project,
            package_name: Some("svc"),
            declared: &deps,
            sibling_packages: &[],
            known_modules: &known,
            source_files: &[],
            local_modules: Default::default(),
            path_aliases: &[],
        };
        PythonProvider.resolve_import(&Import::statement(raw, 1), Utf8Path::new(file), &ctx)
    }

    #[test]
    fn resolves_stdlib_imports() {
        assert_eq!(
            resolve("svc/a.py", "os.path", &[], &[]),
            ImportTarget::Stdlib
        );
        assert_eq!(resolve("svc/a.py", "json", &[], &[]), ImportTarget::Stdlib);
    }

    #[test]
    fn resolves_a_declared_distribution_under_normalization() {
        assert_eq!(
            resolve("svc/a.py", "flask_sqlalchemy", &["Flask-SQLAlchemy"], &[]),
            ImportTarget::External("Flask-SQLAlchemy".to_owned())
        );
    }

    /// The import→package problem: nothing in `yaml` says `PyYAML`.
    #[test]
    fn resolves_curated_import_names_to_their_distribution() {
        assert_eq!(
            resolve("svc/a.py", "yaml", &[], &[]),
            ImportTarget::External("pyyaml".to_owned())
        );
        assert_eq!(
            resolve("svc/a.py", "sklearn.svm", &[], &[]),
            ImportTarget::External("scikit-learn".to_owned())
        );
    }

    /// The translation has to happen before the manifest is consulted, or a
    /// manifest spelling it `PyYAML` collects a false unused-dep *and* a false
    /// missing-dep on the same package.
    #[test]
    fn a_translated_import_matches_the_manifests_own_spelling() {
        assert_eq!(
            resolve("svc/a.py", "yaml", &["PyYAML"], &[]),
            ImportTarget::External("PyYAML".to_owned())
        );
    }

    #[test]
    fn resolves_an_internal_module_by_longest_prefix() {
        assert_eq!(
            resolve(
                "svc/app/a.py",
                "app.api.routes",
                &[],
                &["app", "app.api.routes"]
            ),
            ImportTarget::Internal("app.api.routes".to_owned())
        );
    }

    /// Python resolves a local module before the standard library, and so does this.
    #[test]
    fn a_local_module_shadows_the_standard_library() {
        assert_eq!(
            resolve("svc/a.py", "types", &[], &["types"]),
            ImportTarget::Internal("types".to_owned())
        );
    }

    #[test]
    fn resolves_a_relative_import_to_a_sibling_module() {
        assert_eq!(
            resolve("svc/app/api.py", ".models", &[], &["app.models", "app.api"]),
            ImportTarget::Internal("app.models".to_owned())
        );
    }

    #[test]
    fn resolves_a_parent_relative_import() {
        assert_eq!(
            resolve("svc/app/api/routes.py", "..db", &[], &["app.db"]),
            ImportTarget::Internal("app.db".to_owned())
        );
    }

    /// `from . import thing` where `thing` is a symbol in `__init__.py`, not a
    /// module: the package itself is the dependency.
    #[test]
    fn a_relative_import_of_a_symbol_falls_back_to_the_package() {
        assert_eq!(
            resolve("svc/app/api.py", ".shared_helper", &[], &["app"]),
            ImportTarget::Internal("app".to_owned())
        );
    }

    #[test]
    fn an_over_deep_relative_import_is_unresolved() {
        assert!(matches!(
            resolve("svc/app/api.py", "....far", &[], &["app"]),
            ImportTarget::Unresolved { .. }
        ));
    }

    #[test]
    fn an_undeclared_import_is_external_so_it_is_reported_missing() {
        assert_eq!(
            resolve("svc/a.py", "requests", &[], &[]),
            ImportTarget::External("requests".to_owned())
        );
    }

    // --- version ops ------------------------------------------------------

    #[test]
    fn compares_release_segments() {
        use std::cmp::Ordering;
        assert_eq!(Pep440Ops.compare("1.2.0", "1.10.0"), Some(Ordering::Less));
        assert_eq!(Pep440Ops.compare("2.0", "2.0.0"), Some(Ordering::Less));
        assert_eq!(Pep440Ops.compare("1.2.3", "1.2.3"), Some(Ordering::Equal));
    }

    #[test]
    fn an_unparseable_version_declines_rather_than_guesses() {
        assert_eq!(Pep440Ops.compare("main", "1.0"), None);
    }
}
