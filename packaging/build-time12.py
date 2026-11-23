"""Build the 3036 edition of the stock TIME-12 VST3 from its pinned upstream checkout."""
import argparse
import io
from pathlib import Path
import platform
import shutil
import subprocess
import tarfile
import zipfile


def run(*command, cwd=None):
    subprocess.run([str(arg) for arg in command], cwd=cwd, check=True)


def revision(path):
    return subprocess.check_output(["git", "-C", str(path), "rev-parse", "HEAD"], text=True).strip()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", required=True, type=Path, help="Upstream v1.2.3 checkout with JUCE submodule")
    parser.add_argument("--build-dir", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--cmake", default="cmake")
    args = parser.parse_args()
    upstream = args.source.resolve()
    juce = upstream / "libs/JUCE"
    if revision(upstream) != "cb86fd616c10b8368119b01bcdcabc1654899031" or revision(juce) != "10a589619b452c261b2940767eb253171eb5a823":
        raise ValueError("TIME-12 or JUCE checkout does not match its bundled source revision")

    root = Path(__file__).resolve().parents[1]
    work = args.build_dir.resolve()
    source = work / "src"
    source.mkdir(parents=True, exist_ok=True)
    # Export tracked upstream content, independent of edits in the developer checkout.
    archive = subprocess.check_output(["git", "-C", str(upstream), "archive", "HEAD", "src", "resources", "CMakeLists.txt", "LICENSE"])
    with tarfile.open(fileobj=io.BytesIO(archive)) as tar:
        tar.extractall(source, filter="data")
    run("git", "apply", "--unidiff-zero", "--ignore-space-change", str(root / "crates/os-app/assets/time12/3036-editor.patch"), cwd=source)
    for name in ("StockEditorEffect.h", "StockEditorHost.inc"):
        shutil.copyfile(root / "plugins/lemon-master-assistant/Source/GUI" / name, source / "src" / name)
    cmake = source / "CMakeLists.txt"
    cmake.write_text(cmake.read_text().replace("add_subdirectory(libs/JUCE)", f'add_subdirectory("{juce.as_posix()}" juce)'))
    build = work / "build"
    flags = ["-DBUILD_STANDALONE=OFF", "-DBUILD_LV2=OFF", "-DCMAKE_BUILD_TYPE=Release"]
    if platform.system() == "Darwin":
        flags += ["-DCMAKE_OSX_ARCHITECTURES=arm64;x86_64", "-DCMAKE_OSX_DEPLOYMENT_TARGET=12.0"]
    run(args.cmake, "-S", source, "-B", build, *flags)
    run(args.cmake, "--build", build, "--config", "Release", "--target", "TIME12_VST3", "--parallel", "4")
    bundle = build / "TIME12_artefacts/Release/VST3/TIME-12.vst3"
    if not bundle.is_dir():
        raise RuntimeError("Build did not produce TIME-12.vst3")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    temporary = args.output.with_suffix(".tmp")
    with zipfile.ZipFile(temporary, "w", zipfile.ZIP_DEFLATED) as out:
        for file in sorted(bundle.rglob("*")):
            if file.is_file():
                out.write(file, file.relative_to(bundle).as_posix())
    temporary.replace(args.output)


if __name__ == "__main__":
    main()
