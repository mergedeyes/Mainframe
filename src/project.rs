//! Figures out what "run", "build", "install" etc. mean for the current selection.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use crate::{
    browser::Entry,
    cmd::{Cmd, Step},
    config::Config,
    platform, targets,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Cargo,
    Go,
    Npm,
    Python,
    Zig,
    CMake,
    Meson,
    Make,
    Just,
    /// .NET: a solution or C#/F#/VB project.
    Dotnet,
    /// Visual C++: a .vcxproj, or a solution with one.
    MsBuild,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Cargo => "cargo",
            Kind::Go => "go",
            Kind::Npm => "npm",
            Kind::Python => "python",
            Kind::Zig => "zig",
            Kind::CMake => "cmake",
            Kind::Meson => "meson",
            Kind::Make => "make",
            Kind::Just => "just",
            Kind::Dotnet => "dotnet",
            Kind::MsBuild => "msbuild",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Project {
    pub kind: Kind,
    pub root: PathBuf,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Run,
    Build,
    Release,
    Test,
}

impl Op {
    pub fn verb(self) -> &'static str {
        match self {
            Op::Run => "run",
            Op::Build => "build",
            Op::Release => "release build",
            Op::Test => "test",
        }
    }
}

/// A command and where it runs.
#[derive(Debug, Clone)]
pub struct Plan {
    pub cmd: Cmd,
    pub cwd: PathBuf,
}

/// Files that make a directory a project, most specific first.
const MARKERS: &[(Kind, &[&str])] = &[
    (Kind::Cargo, &["Cargo.toml"]),
    (Kind::Go, &["go.mod"]),
    (Kind::Npm, &["package.json"]),
    (Kind::Python, &["pyproject.toml", "setup.py"]),
    (Kind::Zig, &["build.zig"]),
    (Kind::CMake, &["CMakeLists.txt"]),
    (Kind::Meson, &["meson.build"]),
    (Kind::Make, &["Makefile", "makefile", "GNUmakefile"]),
    (Kind::Just, &["justfile", "Justfile", ".justfile"]),
];

/// Walks up from `start` looking for a project marker. Stops at $HOME so a stray
/// Makefile in your home dir doesn't hijack everything below it.
pub fn detect(start: &Path) -> Option<Project> {
    let home = crate::config::home();
    let mut dir = Some(start);
    while let Some(d) = dir {
        if d == home {
            break;
        }
        if let Some(kind) = kind_at(d) {
            let name = match kind {
                Kind::Cargo => cargo_name(d),
                Kind::Python => targets::python_name(d),
                Kind::Dotnet | Kind::MsBuild => {
                    vs_file(d).and_then(|f| f.file_stem().map(|s| s.to_string_lossy().into_owned()))
                }
                _ => None,
            }
            .or_else(|| d.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_default();
            return Some(Project { kind, root: d.to_path_buf(), name });
        }
        dir = d.parent();
    }
    None
}

/// The build tool whose marker sits right in `dir` (no walking up).
pub fn kind_at(dir: &Path) -> Option<Kind> {
    kinds_at(dir).into_iter().next()
}

/// Every build tool with a marker right in `dir`, most specific first. .NET and MSBuild
/// projects come after cargo and go: an ASP.NET app with a package.json is .NET.
pub fn kinds_at(dir: &Path) -> Vec<Kind> {
    let mut kinds: Vec<Kind> =
        MARKERS.iter().filter(|(_, files)| files.iter().any(|f| dir.join(f).is_file())).map(|&(k, _)| k).collect();
    if let Some(k) = visual_studio_kind(dir) {
        let at = kinds.iter().take_while(|k| matches!(k, Kind::Cargo | Kind::Go)).count();
        kinds.insert(at, k);
    }
    kinds
}

/// Solution and project files: the solution when there is one, else the project.
pub fn vs_file(dir: &Path) -> Option<PathBuf> {
    let files = vs_files(dir);
    let is =
        |p: &&PathBuf, exts: &[&str]| p.extension().is_some_and(|e| exts.iter().any(|x| e.eq_ignore_ascii_case(x)));
    let found = files.iter().find(|p| is(p, &["sln", "slnx"])).or_else(|| files.first());
    found.cloned()
}

fn vs_files(dir: &Path) -> Vec<PathBuf> {
    const EXTS: [&str; 6] = ["sln", "slnx", "csproj", "fsproj", "vbproj", "vcxproj"];
    let Ok(rd) = fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| EXTS.iter().any(|x| e.eq_ignore_ascii_case(x))) && p.is_file())
        .collect();
    files.sort();
    files
}

/// MSBuild when there's C++ (a .vcxproj, or a solution naming one): `dotnet build`
/// can't build those. .NET for the rest.
fn visual_studio_kind(dir: &Path) -> Option<Kind> {
    let files = vs_files(dir);
    if files.is_empty() {
        return None;
    }
    let cpp = files.iter().any(|f| {
        let ext = f.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        ext == "vcxproj"
            || (matches!(ext.as_str(), "sln" | "slnx")
                && fs::read_to_string(f).is_ok_and(|t| t.to_ascii_lowercase().contains(".vcxproj")))
    });
    Some(if cpp { Kind::MsBuild } else { Kind::Dotnet })
}

fn cargo_name(root: &Path) -> Option<String> {
    let text = fs::read_to_string(root.join("Cargo.toml")).ok()?;
    let value: toml::Table = toml::from_str(&text).ok()?;
    Some(value.get("package")?.get("name")?.as_str()?.to_string())
}

/// What `op` means for `target` (the selected entry), or for `cwd` when nothing is selected.
pub fn plan(op: Op, target: Option<&Entry>, cwd: &Path, cfg: &Config) -> Result<Plan, String> {
    let file = target.filter(|e| !e.is_dir);
    let start = match target {
        Some(e) if e.is_dir => e.path.clone(),
        _ => cwd.to_path_buf(),
    };
    let project = detect(&start);

    if let Some(f) = file
        && let Some(plan) = file_plan(op, f, project.as_ref(), cfg)
    {
        return Ok(plan);
    }

    let Some(p) = project else {
        return Err(match file {
            Some(f) => format!("don't know how to {} {}", op.verb(), f.name),
            None => format!("no project here to {}", op.verb()),
        });
    };
    if p.kind == Kind::Python {
        return targets::python_plan(op, &p.root).map(|cmd| Plan { cmd, cwd: p.root.clone() });
    }
    let w = Cmd::words;
    let cmd = match (p.kind, op) {
        (Kind::Cargo, Op::Run) => w("cargo run"),
        (Kind::Cargo, Op::Build) => w("cargo build"),
        (Kind::Cargo, Op::Release) => w("cargo build --release"),
        (Kind::Cargo, Op::Test) => w("cargo test"),
        (Kind::Go, Op::Run) => w("go run ."),
        (Kind::Go, Op::Build | Op::Release) => w("go build ./..."),
        (Kind::Go, Op::Test) => w("go test ./..."),
        (Kind::Npm, Op::Run) => w("npm start"),
        (Kind::Npm, Op::Build | Op::Release) => w("npm run build"),
        (Kind::Npm, Op::Test) => w("npm test"),
        (Kind::Make, Op::Run) => w("make run"),
        (Kind::Make, Op::Build | Op::Release) => w("make"),
        (Kind::Make, Op::Test) => w("make test"),
        (Kind::Zig, Op::Run) => w("zig build run"),
        (Kind::Zig, Op::Build) => w("zig build"),
        (Kind::Zig, Op::Release) => w("zig build -Doptimize=ReleaseFast"),
        (Kind::Zig, Op::Test) => w("zig build test"),
        (Kind::CMake, Op::Run) => return Err("cmake doesn't say what to run — T lists its tasks".into()),
        // --config and -C pick the build type for multi-config generators (Visual Studio,
        // Ninja Multi-Config); single-config ones ignore them.
        (Kind::CMake, Op::Build) => w("cmake -S . -B build").then(Step::words("cmake --build build --config Debug")),
        (Kind::CMake, Op::Release) => w("cmake -S . -B build-release -DCMAKE_BUILD_TYPE=Release")
            .then(Step::words("cmake --build build-release --config Release")),
        (Kind::CMake, Op::Test) => w("ctest --test-dir build --output-on-failure -C Debug"),
        (Kind::Meson, Op::Run) => return Err("meson doesn't say what to run — T lists its tasks".into()),
        (Kind::Meson, Op::Build) => {
            Step::words("meson setup build").unless_exists("build").then(Step::words("meson compile -C build"))
        }
        (Kind::Meson, Op::Release) => Step::words("meson setup build-release --buildtype=release")
            .unless_exists("build-release")
            .then(Step::words("meson compile -C build-release")),
        (Kind::Meson, Op::Test) => w("meson test -C build"),
        (Kind::Just, Op::Run) => w("just"),
        (Kind::Just, Op::Build | Op::Release) => w("just build"),
        (Kind::Just, Op::Test) => w("just test"),
        (Kind::Dotnet, Op::Run) => match targets::dotnet_runnable(&p.root).as_slice() {
            [] => return Err("nothing here dotnet can run — T lists its tasks".into()),
            [one] if *one == p.root => w("dotnet run"),
            [one] => Step::words("dotnet run --project").path(one).into(),
            _ => return Err("more than one project can run — T lists them".into()),
        },
        (Kind::Dotnet, Op::Build) => w("dotnet build"),
        (Kind::Dotnet, Op::Release) => w("dotnet build -c Release"),
        (Kind::Dotnet, Op::Test) => w("dotnet test"),
        (Kind::MsBuild, Op::Build) => msbuild(&p.root, "Debug", None)?,
        (Kind::MsBuild, Op::Release) => msbuild(&p.root, "Release", None)?,
        (Kind::MsBuild, Op::Run | Op::Test) => {
            return Err(format!("msbuild doesn't say what to {} — T lists its tasks", op.verb()));
        }
        (Kind::Python, _) => unreachable!(),
    };
    Ok(Plan { cmd, cwd: p.root })
}

fn file_plan(op: Op, f: &Entry, project: Option<&Project>, cfg: &Config) -> Option<Plan> {
    let dir = f.path.parent().unwrap_or(Path::new("/")).to_path_buf();
    let ext = f.path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();

    if op != Op::Run {
        return None;
    }

    // A .rs file inside a cargo project: run its bin/example target.
    if let Some(p) = project.filter(|p| p.kind == Kind::Cargo && ext == "rs") {
        let stem = f.path.file_stem()?.to_string_lossy();
        let parent = f.path.parent()?;
        let mut run = Step::words("cargo run");
        if parent == p.root.join("src").join("bin") {
            run = run.args(["--bin", &stem]);
        } else if parent == p.root.join("examples") {
            run = run.args(["--example", &stem]);
        }
        return Some(Plan { cmd: run.into(), cwd: p.root.clone() });
    }

    if let Some(tpl) = cfg.runners.get(&ext) {
        return Some(Plan { cmd: Cmd::Shell(expand_file(tpl, f, &cfg.shell())), cwd: dir });
    }

    // Commands run from the file's own directory, so the bare name is enough.
    let name = f.name.as_str();
    let exe =
        env::temp_dir().join(format!("mainframe-{}{}", f.path.file_stem()?.to_string_lossy(), env::consts::EXE_SUFFIX));
    let exe_s = exe.to_string_lossy().into_owned();
    let cmd: Cmd = match ext.as_str() {
        "sh" | "bash" => Step::new(bash()).arg(name).into(),
        "zsh" | "fish" => Step::new(ext.as_str()).arg(name).into(),
        "ps1" => powershell_file(name).into(),
        "py" => python().arg(name).into(),
        "js" | "mjs" | "cjs" => Step::new("node").arg(name).into(),
        "ts" => Step::words("npx --yes tsx").arg(name).into(),
        "rb" => Step::new("ruby").arg(name).into(),
        "lua" => Step::new("lua").arg(name).into(),
        "pl" => Step::new("perl").arg(name).into(),
        "php" => Step::new("php").arg(name).into(),
        "go" => Step::words("go run").arg(name).into(),
        "rs" => Step::words("rustc --edition 2024 -o").args([&exe_s, name]).then(Step::new(&exe_s)),
        "c" => compile_c(false, name, &exe).then(Step::new(&exe_s)),
        "cpp" | "cc" | "cxx" => compile_c(true, name, &exe).then(Step::new(&exe_s)),
        _ if f.is_exec && cfg!(windows) => Step::new(f.path.to_string_lossy()).into(),
        _ if f.is_exec => Step::new(format!("./{name}")).into(),
        _ => return None,
    };
    Some(Plan { cmd, cwd: dir })
}

/// msbuild on the solution (or project) in `root`, in `config`, with `target` (Clean,
/// Rebuild) when given. Full paths in messages, so jumping to an error works from anywhere.
pub fn msbuild(root: &Path, config: &str, target: Option<&str>) -> Result<Cmd, String> {
    let file = vs_file(root).ok_or("no solution or project file here")?;
    let name = file.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let mut step = Step::new("msbuild").arg(name).args(["-nologo", "-m", "-p:GenerateFullPaths=true"]);
    step = step.arg(format!("-p:Configuration={config}"));
    if let Some(t) = target {
        step = step.arg(format!("-t:{t}"));
    }
    Ok(step.into())
}

/// Python 3: `python3` on Unix; on Windows the `py` launcher when it's there (`python3` there
/// may be the Store's placeholder that opens the Store).
pub fn python() -> Step {
    if !cfg!(windows) {
        Step::new("python3")
    } else if platform::which("py").is_some() {
        Step::new("py").arg("-3")
    } else {
        Step::new("python")
    }
}

/// On Windows, Git's bash: the `bash` on PATH there is often WSL's.
fn bash() -> String {
    platform::git_sh("bash").map_or_else(|| "bash".into(), |p| p.to_string_lossy().into_owned())
}

fn powershell_file(name: &str) -> Step {
    let ps = if platform::which("pwsh").is_some() { "pwsh" } else { "powershell" };
    let step = Step::new(ps).args(["-NoLogo", "-NoProfile"]);
    let step = if cfg!(windows) { step.args(["-ExecutionPolicy", "Bypass"]) } else { step };
    step.args(["-File", name])
}

/// Compiles one C or C++ file to `exe`. On Windows: cl when Visual Studio is there (tasks
/// get its environment), else clang, else gcc (MinGW), else cl anyway.
fn compile_c(cpp: bool, name: &str, exe: &Path) -> Step {
    let exe_s = exe.to_string_lossy();
    let unix = |cc: &str| Step::new(cc).args(["-Wall", "-o", &exe_s, name]);
    if !cfg!(windows) {
        return unix(if cpp { "c++" } else { "cc" });
    }
    let has = |p: &str| platform::which(p).is_some();
    if !platform::msvc_available() {
        let (clang, gcc) = if cpp { ("clang++", "g++") } else { ("clang", "gcc") };
        if has(clang) {
            return unix(clang);
        }
        if has(gcc) {
            return unix(gcc);
        }
    }
    // /Fo keeps the .obj out of the source folder.
    let obj_dir = format!("{}{}", exe.parent().unwrap_or(Path::new(".")).display(), std::path::MAIN_SEPARATOR);
    let step = Step::words("cl /nologo /W4");
    let step = if cpp { step.arg("/EHsc") } else { step };
    step.arg(format!("/Fe:{exe_s}")).arg(format!("/Fo:{obj_dir}")).arg(name)
}

fn expand_file(tpl: &str, f: &Entry, shell: &crate::shell::Shell) -> String {
    tpl.replace("{file}", &shell.quote_path(&f.path)).replace("{name}", &shell.quote(&f.name))
}

pub fn quote(p: &Path) -> String {
    quote_str(&p.to_string_lossy())
}

/// POSIX quoting, for sh that runs on Unix: the deploy steps on the other end, the sudo
/// install of a .desktop entry. What the user's shell runs is quoted by `Shell::quote`.
pub fn quote_str(s: &str) -> String {
    crate::shell::posix_quote(s)
}

pub use crate::platform::which;

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    use crate::{run::testing::run_in, testutil::temp_dir};

    fn entry(path: &Path) -> Entry {
        Entry {
            name: path.file_name().unwrap().to_string_lossy().into_owned(),
            path: path.to_path_buf(),
            is_dir: false,
            is_exec: false,
            is_symlink: false,
            size: 0,
            modified: None,
        }
    }

    /// Builds with the plans `b` and `R` use, in Visual Studio's environment.
    #[test]
    fn cmake_builds_debug_and_release_with_msvc() {
        if !platform::msvc_available() || platform::which("cmake").is_none() {
            return;
        }
        let dir = temp_dir("cmake-msvc");
        fs::write(
            dir.join("CMakeLists.txt"),
            "cmake_minimum_required(VERSION 3.20)\nproject(hello C)\nadd_executable(hello main.c)\n",
        )
        .unwrap();
        fs::write(dir.join("main.c"), "#include <stdio.h>\nint main(void) { puts(\"hi\"); return 0; }\n").unwrap();
        let cfg = Config::default();
        for (op, exe) in [(Op::Build, r"build\Debug\hello.exe"), (Op::Release, r"build-release\Release\hello.exe")] {
            let plan = plan(op, None, &dir, &cfg).unwrap();
            let (ok, out) = run_in(&plan.cmd, &plan.cwd);
            assert!(ok, "{out}");
            assert!(dir.join(exe).is_file(), "no {exe}:\n{out}");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    /// A console app: `b` builds it, `r` runs it.
    #[test]
    fn dotnet_builds_and_runs() {
        if platform::which("dotnet").is_none() {
            return;
        }
        let dir = temp_dir("dotnet-run");
        fs::write(dir.join("Hello.csproj"), super::visual_studio_tests::CONSOLE).unwrap();
        fs::write(dir.join("Program.cs"), "System.Console.WriteLine(\"hello from dotnet\");\n").unwrap();
        let cfg = Config::default();
        for op in [Op::Build, Op::Run] {
            let plan = plan(op, None, &dir, &cfg).unwrap();
            let (ok, out) = run_in(&plan.cmd, &plan.cwd);
            assert!(ok, "{out}");
            if op == Op::Run {
                assert!(out.contains("hello from dotnet"), "{out}");
            }
        }
        let _ = fs::remove_dir_all(&dir);
    }

    /// A Visual Studio solution (made by CMake's generator): `b` and `R` build it with msbuild.
    #[test]
    fn msbuild_builds_a_cpp_solution() {
        if !platform::msvc_available() || platform::which("cmake").is_none() {
            return;
        }
        let dir = temp_dir("msbuild-run");
        let src = dir.join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(
            src.join("CMakeLists.txt"),
            "cmake_minimum_required(VERSION 3.20)\nproject(game C)\nadd_executable(game main.c)\n",
        )
        .unwrap();
        fs::write(src.join("main.c"), "int main(void) { return 0; }\n").unwrap();
        let vs = dir.join("vs");
        let generate = Step::new("cmake").arg("-S").path(&src).arg("-B").path(&vs);
        let (ok, out) = run_in(&generate.into(), &dir);
        assert!(ok, "{out}");
        let p = detect(&vs).unwrap();
        assert_eq!(
            p.kind,
            Kind::MsBuild,
            "{:?}",
            fs::read_dir(&vs).unwrap().flatten().map(|e| e.file_name()).collect::<Vec<_>>()
        );
        let cfg = Config::default();
        for (op, exe) in [(Op::Build, r"Debug\game.exe"), (Op::Release, r"Release\game.exe")] {
            let plan = plan(op, None, &vs, &cfg).unwrap();
            let (ok, out) = run_in(&plan.cmd, &plan.cwd);
            assert!(ok, "{out}");
            assert!(vs.join(exe).is_file(), "no {exe}:\n{out}");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    /// `r` on a C file: compiled with cl, then run.
    #[test]
    fn runs_a_c_file_with_cl() {
        if !platform::msvc_available() {
            return;
        }
        let dir = temp_dir("c-file");
        let file = dir.join("hello.c");
        fs::write(&file, "#include <stdio.h>\nint main(void) { puts(\"hello from cl\"); return 0; }\n").unwrap();
        let plan = plan(Op::Run, Some(&entry(&file)), &dir, &Config::default()).unwrap();
        let (ok, out) = run_in(&plan.cmd, &plan.cwd);
        let _ = fs::remove_dir_all(&dir);
        assert!(ok, "{out}");
        assert!(out.contains("hello from cl"), "{out}");
        assert!(plan.cmd.display(&crate::shell::Shell::default()).starts_with("cl "), "{out}");
    }
}

#[cfg(test)]
pub(super) mod visual_studio_tests {
    use super::*;
    use crate::{shell::Shell, testutil::temp_dir};

    pub const CONSOLE: &str = "<Project Sdk=\"Microsoft.NET.Sdk\">\n  <PropertyGroup>\n    <OutputType>Exe</OutputType>\n    <TargetFramework>net10.0</TargetFramework>\n  </PropertyGroup>\n</Project>\n";
    const LIBRARY: &str = "<Project Sdk=\"Microsoft.NET.Sdk\">\n  <PropertyGroup>\n    <TargetFramework>net10.0</TargetFramework>\n  </PropertyGroup>\n</Project>\n";

    fn shown(cmd: &Cmd) -> String {
        cmd.display(&Shell::sh())
    }

    #[test]
    fn dotnet_projects_and_solutions() {
        let dir = temp_dir("dotnet-kinds");
        let (app, lib) = (dir.join("App"), dir.join("Lib"));
        fs::create_dir_all(&app).unwrap();
        fs::create_dir_all(&lib).unwrap();
        fs::write(app.join("App.csproj"), CONSOLE).unwrap();
        fs::write(lib.join("Lib.csproj"), LIBRARY).unwrap();
        fs::write(dir.join("Demo.sln"), "Project(\"{FAE}\") = \"App\", \"App\\App.csproj\", \"{1}\"\nEndProject\n")
            .unwrap();
        // An ASP.NET app with a frontend is .NET first.
        fs::write(dir.join("package.json"), "{}").unwrap();

        assert_eq!(kinds_at(&dir), [Kind::Dotnet, Kind::Npm]);
        let p = detect(&dir).unwrap();
        assert_eq!((p.kind, p.name.as_str()), (Kind::Dotnet, "Demo"));
        assert_eq!(targets::dotnet_runnable(&dir), std::slice::from_ref(&app));
        let cfg = Config::default();
        let run = plan(Op::Run, None, &dir, &cfg).unwrap();
        assert!(shown(&run.cmd).starts_with("dotnet run --project "), "{}", shown(&run.cmd));
        assert_eq!(shown(&plan(Op::Release, None, &dir, &cfg).unwrap().cmd), "dotnet build -c Release");
        assert_eq!(shown(&plan(Op::Run, None, &app, &cfg).unwrap().cmd), "dotnet run");
        assert!(plan(Op::Run, None, &lib, &cfg).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cpp_solutions_build_with_msbuild() {
        let dir = temp_dir("msbuild-kinds");
        fs::write(dir.join("Game.sln"), "Project(\"{8BC}\") = \"Game\", \"Game\\Game.vcxproj\", \"{2}\"\nEndProject\n")
            .unwrap();
        let p = detect(&dir).unwrap();
        assert_eq!(p.kind, Kind::MsBuild);
        let cfg = Config::default();
        assert_eq!(
            shown(&plan(Op::Release, None, &dir, &cfg).unwrap().cmd),
            "msbuild Game.sln -nologo -m -p:GenerateFullPaths=true -p:Configuration=Release"
        );
        assert!(plan(Op::Run, None, &dir, &cfg).is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}
