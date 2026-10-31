"""使用本地已有依赖，准备公开样本并运行 Decoder 离线对照探针。

示例：python3 run.py --rime-root /path/to/librime/1.16.1 \
    --dictionary /path/to/public/pinyin_simp.dict.yaml

--scripts 可指向上次 Y0_OUTPUT/scripts，令 Rime 和 Mock 使用完全相同的
已解析脚本。所有新资源、用户目录和证据留在临时目录；不清理运行证据。
--tested zhixu 执行完整组词与编辑套件，不跳过脚本；--typing-only --count 300
生成公开纯输入套件，统计每个完整输入最终第一页的候选结构。
--session 在 Session + Mixer 层比较旧整串开关路径与知序内层；--tested rime
在该模式下执行旧路径对自身的自检。Y0 的内层动作转换为实际宿主按键，
不把内层候选结构差异当作用户可见结果。
"""

import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile

import generate


def validate_inputs(rime_root, dictionary, scripts=None):
    rime_root = Path(rime_root).resolve()
    dictionary = Path(dictionary).resolve()
    scripts = Path(scripts).resolve() if scripts is not None else None
    if not (rime_root / "include/rime_api.h").is_file() or not (rime_root / "lib").is_dir():
        raise ValueError("--rime-root must contain existing include/rime_api.h and lib/")
    libraries = [path for path in (rime_root / "lib").iterdir()
                 if path.is_file() and (path.name.startswith("librime.") or path.name == "rime.lib")]
    if not libraries:
        raise ValueError("--rime-root/lib has no existing librime library")
    if not dictionary.is_file() or dictionary.name != "pinyin_simp.dict.yaml":
        raise ValueError("--dictionary must be an existing public pinyin_simp.dict.yaml")
    header = dictionary.read_text(encoding="utf-8-sig").split("\n...", 1)[0]
    if not re.search(r"^name:\s*[\"']?pinyin_simp[\"']?\s*$", header, re.MULTILINE):
        raise ValueError("--dictionary must declare name: pinyin_simp")
    if scripts is not None:
        valid = (scripts.is_file() and scripts.suffix == ".keys") or (
            scripts.is_dir() and any(scripts.rglob("*.keys")))
        if not valid:
            raise ValueError("--scripts must be a .keys file or a directory containing .keys files")
    return rime_root, dictionary, scripts


def prepare_shared(repository, dictionary, destination):
    repository, dictionary, destination = map(Path, (repository, dictionary, destination))
    destination.mkdir(parents=True, exist_ok=False)
    resources = repository / "native/resources/Rime"
    for name in ["default.yaml", "agent_pinyin.schema.yaml"]:
        shutil.copyfile(resources / name, destination / name)
    shutil.copyfile(dictionary, destination / "pinyin_simp.dict.yaml")
    (destination / "agent_pinyin.dict.yaml").write_text(
        "# Y0 public-only fixture; no product or owner tables.\n"
        "---\nname: agent_pinyin\nversion: \"y0-public\"\nsort: by_weight\n"
        "import_tables:\n  - pinyin_simp\n...\n", encoding="utf-8")
    return destination


def shim_from_message(message):
    package = str(message.get("package_id", ""))
    if message.get("reason") != "build-script-executed" or not (
            "#zhizi-host@" in package or package.startswith("zhizi-host ")):
        return None
    out = Path(message["out_dir"])
    if not (out / "libaim_rime_shim.a").is_file():
        raise ValueError(f"host build-script output contains no libaim_rime_shim.a: {out}")
    return out


def logged_command(command, cwd, environment, log_path, on_line):
    with log_path.open("w", encoding="utf-8") as log:
        process = subprocess.Popen(command, cwd=cwd, env=environment, stdout=subprocess.PIPE,
                                   stderr=subprocess.STDOUT, text=True, encoding="utf-8")
        for line in process.stdout:
            log.write(line)
            on_line(line)
        status = process.wait()
    if status:
        raise RuntimeError(f"command exited {status}; see {log_path}")


def arguments(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rime-root", required=True, type=Path)
    parser.add_argument("--dictionary", required=True, type=Path)
    parser.add_argument("--tested", choices=["rime", "mock", "zhixu"], default="rime")
    parser.add_argument("--session", action="store_true", help="比较 Session + Mixer 的用户可见结果")
    parser.add_argument("--scripts", type=Path)
    parser.add_argument("--typing-only", action="store_true", help="仅生成公开逐键输入；统计最终完整输入第一页结构")
    parser.add_argument("--count", type=int, help="公开输入数；普通套件默认70，只输入套件默认300")
    args = parser.parse_args(argv)
    if args.session and args.tested == "mock":
        parser.error("--session supports --tested rime (self-check) or zhixu")
    if args.scripts is not None and (args.typing_only or args.count is not None):
        parser.error("--scripts cannot be combined with --typing-only or --count")
    if args.count is None:
        args.count = 300 if args.typing_only else 70
    if args.count <= 0:
        parser.error("--count must be positive")
    return args


def probe_name(args):
    return "session_contract_compare" if args.session else "decoder_contract_compare"


def main(argv=None):
    args = arguments(argv)
    try:
        rime_root, dictionary, scripts = validate_inputs(args.rime_root, args.dictionary, args.scripts)
    except (OSError, ValueError) as error:
        raise ValueError(str(error)) from error
    repository = Path(__file__).resolve().parents[4]
    temporary = Path(tempfile.mkdtemp(prefix="zhixu-y3-run-" if args.session else "zhixu-y0-run-"))
    print(f"Y0_RUN\t{temporary}", flush=True)
    shared = prepare_shared(repository, dictionary, temporary / "shared")
    if scripts is None:
        scripts = temporary / "suite"
        scripts.mkdir()
        if not args.typing_only:
            shutil.copytree(Path(__file__).parent / "fixed", scripts / "fixed")
        generation = ["--dictionary", str(dictionary), "--output", str(scripts / "public"), "--count", str(args.count)]
        if args.typing_only:
            generation.append("--typing-only")
        generate.main(generation)

    environment = os.environ.copy()
    environment["RIME_ROOT"] = str(rime_root)
    environment["DYLD_LIBRARY_PATH"] = str(rime_root / "lib")
    environment["LD_LIBRARY_PATH"] = str(rime_root / "lib")
    # A previous probe's linker flags may point to another target directory.
    for name in ["RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS"]:
        environment.pop(name, None)
    host = repository / "native/host"
    shim_dirs = []

    def build_line(line):
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            return
        out = shim_from_message(message)
        if out is not None:
            shim_dirs.append(out)

    build_command = ["cargo", "build", "--offline", "--lib", "--message-format=json"]
    probe_command = ["cargo", "test", "--offline", "--features", "librime",
                     probe_name(args), "--", "--ignored", "--nocapture"]
    print(f"Y0_BUILD_LOG\t{temporary / 'build.log'}", flush=True)
    logged_command(build_command, host, environment, temporary / "build.log", build_line)
    if not shim_dirs:
        raise RuntimeError(f"cargo emitted no zhizi-host build-script-executed output; see {temporary / 'build.log'}")
    shim = shim_dirs[-1]
    flags = ["-L", f"native={shim}", "-l", "static=aim_rime_shim",
             "-L", f"native={rime_root / 'lib'}", "-l", "dylib=rime"]
    environment["RUSTFLAGS"] = " ".join(flags)
    # Encoded flags take precedence and also support spaces in dependency paths.
    environment["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join(flags)
    environment["Y0_SHARED"] = str(shared)
    environment["Y0_SCRIPTS"] = str(scripts)
    environment["Y0_TESTED"] = args.tested
    if args.session:
        environment["Y3_TESTED"] = "legacy" if args.tested == "rime" else "zhixu"
    record = {"tested": args.tested, "shared": str(shared), "scripts": str(scripts),
              "rime_root": str(rime_root), "shim": str(shim), "build": build_command,
              "probe": probe_command, "typing_only": args.typing_only, "session": args.session,
              "dictionary_sha256": generate.sha256(dictionary)}
    (temporary / "run.json").write_text(json.dumps(record, ensure_ascii=False, indent=2) + "\n")

    def probe_line(line):
        if "Y0_" in line or "Y1_" in line or "Y2_" in line or "Y3_" in line:
            print(line.rstrip(), flush=True)

    print(f"Y0_PROBE_LOG\t{temporary / 'probe.log'}", flush=True)
    logged_command(probe_command, repository / "native/core/agent-ime-core", environment,
                   temporary / "probe.log", probe_line)
    print(f"Y0_DONE\tlogs={temporary}\ttested={args.tested}", flush=True)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, RuntimeError) as error:
        print(f"Y0_ERROR\t{error}", file=sys.stderr)
        sys.exit(1)
