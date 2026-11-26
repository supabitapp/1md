#!/usr/bin/env python3
import argparse
import json
import pathlib
import shutil
import subprocess
import tempfile
import tomllib

CRATES = ["clock", "collections", "gpui_util", "rope", "sum_tree", "text", "ztracing", "ztracing_macro"]
ROOT = pathlib.Path(__file__).resolve().parent.parent
VENDOR = ROOT / "vendor" / "zed"
PATCHES = VENDOR / "patches"
REVISION = VENDOR / "REVISION"


def fetch(repo: str, rev: str, dest: pathlib.Path) -> str:
    def git(*args: str) -> str:
        return subprocess.run(["git", "-C", str(dest), *args], check=True, capture_output=True, text=True).stdout

    git("init", "-q")
    git("fetch", "-q", "--depth", "1", "--filter=blob:none", repo, rev)
    paths = ["Cargo.toml", "LICENSE-GPL", "LICENSE-APACHE", *(f"crates/{c}" for c in CRATES)]
    git("checkout", "-q", "FETCH_HEAD", "--", *paths)
    return git("rev-parse", "FETCH_HEAD").strip()


def resolve(name: str, spec, workspace: dict):
    if isinstance(spec, str):
        return {"version": spec}
    spec = dict(spec)
    if not spec.pop("workspace", False):
        return spec
    base = workspace[name]
    base = {"version": base} if isinstance(base, str) else dict(base)
    if "path" in base:
        if name not in CRATES:
            return None
        base = {"path": f"../{name}"}
    features = [*base.pop("features", []), *spec.pop("features", [])]
    merged = {**base, **spec}
    if features:
        merged["features"] = list(dict.fromkeys(features))
    return merged


def feature_target(item: str) -> str:
    return item.removeprefix("dep:").split("/")[0].removesuffix("?")


def prune(features: dict, deps: dict) -> dict:
    kept = dict(features)
    changed = True
    while changed:
        changed = False
        for name, items in list(kept.items()):
            if any(feature_target(i) not in kept and feature_target(i) not in deps for i in items):
                del kept[name]
                changed = True
    return kept


def used_optional(features: dict) -> set:
    return {feature_target(i) for items in features.values() for i in items}


def toml_value(v) -> str:
    if isinstance(v, bool):
        return "true" if v else "false"
    if isinstance(v, int):
        return str(v)
    if isinstance(v, str):
        return json.dumps(v)
    if isinstance(v, list):
        return "[" + ", ".join(toml_value(x) for x in v) + "]"
    return "{ " + ", ".join(f"{k} = {toml_value(x)}" for k, x in v.items()) + " }"


def dep_line(name: str, spec: dict) -> str:
    if list(spec) == ["version"]:
        return f"{name} = {toml_value(spec['version'])}"
    return f"{name} = {toml_value(spec)}"


def manifest(crate: dict, workspace: dict, lints: dict) -> str:
    package = crate["package"]
    tables = {"dependencies": crate.get("dependencies", {})}
    for target, sections in crate.get("target", {}).items():
        if "dependencies" in sections:
            tables[f"target.'{target}'.dependencies"] = sections["dependencies"]
    resolved = {
        table: {n: r for n, s in deps.items() if (r := resolve(n, s, workspace)) is not None}
        for table, deps in tables.items()
    }
    all_deps = {n: s for deps in resolved.values() for n, s in deps.items()}
    features = prune(crate.get("features", {}), all_deps)
    wanted = used_optional(features)
    lines = [
        "[package]",
        f"name = {toml_value(package['name'])}",
        f"version = {toml_value(package['version'])}",
        'edition = "2024"',
        "publish = false",
        f"license = {toml_value(package['license'])}",
    ]
    if "lib" in crate:
        lines += ["", "[lib]", *(f"{k} = {toml_value(v)}" for k, v in crate["lib"].items())]
    if features:
        lines += ["", "[features]", *(f"{k} = {toml_value(v)}" for k, v in features.items())]
    for table, deps in resolved.items():
        kept = {n: s for n, s in deps.items() if not s.get("optional") or n in wanted}
        if kept:
            lines += ["", f"[{table}]", *(dep_line(n, s) for n, s in sorted(kept.items()))]
    if crate.get("lints", {}).get("workspace"):
        for tool in ["rust", "clippy"]:
            if tool in lints:
                lines += ["", f"[lints.{tool}]", *(f"{k} = {toml_value(v)}" for k, v in lints[tool].items())]
    return "\n".join(lines) + "\n"


def vendor(source: pathlib.Path) -> None:
    root = tomllib.loads((source / "Cargo.toml").read_text())["workspace"]
    workspace, lints = root["dependencies"], root.get("lints", {})
    for name in CRATES:
        src = source / "crates" / name
        dest = VENDOR / name
        shutil.rmtree(dest, ignore_errors=True)
        shutil.copytree(src / "src", dest / "src")
        for extra in ["build.rs", *(p.name for p in src.glob("LICENSE*"))]:
            if (src / extra).exists():
                shutil.copy(src / extra, dest / extra)
        crate = tomllib.loads((src / "Cargo.toml").read_text())
        (dest / "Cargo.toml").write_text(manifest(crate, workspace, lints))


def apply_patches() -> None:
    for patch in sorted(PATCHES.glob("*.patch")):
        subprocess.run(["git", "apply", f"--directory={VENDOR.relative_to(ROOT)}", str(patch)], cwd=ROOT, check=True)


def main() -> None:
    parser = argparse.ArgumentParser(description="Vendor Zed's text CRDT crates into vendor/zed and apply vendor/zed/patches.")
    parser.add_argument("--rev", help="Zed commit to vendor (defaults to vendor/zed/REVISION)")
    parser.add_argument("--repo", default="https://github.com/zed-industries/zed", help="Zed git repository URL or path")
    args = parser.parse_args()
    rev = args.rev or REVISION.read_text().strip()
    with tempfile.TemporaryDirectory() as tmp:
        source = pathlib.Path(tmp)
        sha = fetch(args.repo, rev, source)
        vendor(source)
    apply_patches()
    REVISION.write_text(sha + "\n")
    print(f"vendored zed {sha}")


if __name__ == "__main__":
    main()
