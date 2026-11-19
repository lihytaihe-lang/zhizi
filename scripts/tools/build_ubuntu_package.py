#!/usr/bin/env python3
"""Build a local amd64 IBus deb from an already compiled, pinned payload."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--root', type=Path, required=True)
    ap.add_argument('--payload', type=Path, required=True)
    ap.add_argument('--binaries', type=Path, required=True)
    ap.add_argument('--output', type=Path, required=True)
    ap.add_argument('--source-commit', required=True)
    ap.add_argument('--package-version')
    args = ap.parse_args()
    root = args.root.resolve()
    payload = args.payload.resolve()
    version = (root / 'VERSION').read_text().strip()
    package_version = args.package_version or version
    if not re.fullmatch(r'\d+\.\d+\.\d+(?:~rc\d+)?', package_version):
        ap.error('invalid package version')
    if not re.fullmatch(r'[a-f0-9]{40}', args.source_commit):
        ap.error('source commit must be a full git hash')
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    stage = output / ('stage-' + package_version)
    if stage.exists():
        ap.error('requires a new stage directory')
    stage.mkdir()
    share = stage / 'usr/share/zhizi'
    lib = stage / 'usr/lib/zhizi'
    lib.mkdir(parents=True)
    share.mkdir(parents=True)
    manifest = json.loads((payload / 'ModelManifest.json').read_text())
    for group, directory in [('ranking', 'Model'), ('generation', 'ZhijuModel')]:
        for name, expected in manifest[group]['files'].items():
            if Path(name).name != name:
                ap.error('unsafe model manifest path')
            if hashlib.sha256((payload / directory / name).read_bytes()).hexdigest() != expected:
                ap.error('fixed model payload does not match manifest: ' + name)
    for line in (root / 'native/resources/WordBigram/SHA256SUMS').read_text().splitlines():
        expected, relative = line.split()
        if '..' in Path(relative).parts or Path(relative).is_absolute():
            ap.error('unsafe word table manifest')
        if hashlib.sha256((payload / 'Rime/word-bigram' / relative).read_bytes()).hexdigest() != expected:
            ap.error('word table does not match pinned manifest: ' + relative)
    for directory in ['Rime', 'Model', 'ZhijuModel', 'Licenses']:
        shutil.copytree(payload / directory, share / directory)
    shutil.copyfile(payload / 'ModelManifest.json', share / 'ModelManifest.json')
    for name in ['zhizi-ibus-host', 'zhizi-learning']:
        shutil.copyfile(args.binaries / name, lib / name)
        (lib / name).chmod(0o755)
    for path in (payload / 'lib').iterdir():
        if path.name.startswith('libonnxruntime') and '.so' in path.name:
            if path.is_symlink():
                os.symlink(os.readlink(path), lib / path.name)
            elif path.is_file():
                shutil.copyfile(path, lib / path.name)
                (lib / path.name).chmod(0o755)
    module = root / 'native/linux/ibus-zhizi'
    (share / 'ibus').mkdir()
    for name in ['engine.py', 'bridge.py', 'keys.py', 'settings.py', 'messages.py']:
        shutil.copyfile(module / name, share / 'ibus' / name)
    wrappers = {
        'usr/lib/ibus/ibus-engine-zhizi': '#!/bin/sh\nexec /usr/bin/python3 /usr/share/zhizi/ibus/engine.py "$@"\n',
        'usr/bin/zhizi-settings': '#!/bin/sh\nexec /usr/bin/python3 /usr/share/zhizi/ibus/settings.py "$@"\n',
    }
    for relative, text in wrappers.items():
        path = stage / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
        path.chmod(0o755)
    component = stage / 'usr/share/ibus/component/zhizi.xml'
    component.parent.mkdir(parents=True)
    component.write_text((module / 'zhizi.xml.in').read_text().replace('@VERSION@', version))
    desktop = stage / 'usr/share/applications/zhizi-settings.desktop'
    desktop.parent.mkdir(parents=True)
    shutil.copyfile(module / 'zhizi-settings.desktop', desktop)
    icon = stage / 'usr/share/icons/hicolor/128x128/apps/zhizi.png'
    icon.parent.mkdir(parents=True)
    shutil.copyfile(root / 'src-tauri/icons/128x128.png', icon)
    documentation = stage / 'usr/share/doc/ibus-zhizi'
    documentation.mkdir(parents=True)
    shutil.copyfile(root / 'native/linux/README.zh-CN.md', documentation / 'README.zh-CN.md')
    source = {'version': version, 'packageVersion': package_version, 'sourceCommit': args.source_commit,
              'nativeABI': 6, 'coreVersion': '0.3.1', 'modelManifestSHA256': hashlib.sha256((share / 'ModelManifest.json').read_bytes()).hexdigest(),
              'newTraining': False, 'recordsOrUploadEnabled': False}
    (documentation / 'BUILD-SOURCE.json').write_text(json.dumps(source, indent=2) + '\n')
    (documentation / 'copyright').write_text('Project and native core: Apache-2.0 / MIT.\nBundled model vocabulary and dictionaries retain their upstream terms.\nFull license texts and notices: /usr/share/zhizi/Licenses/\nThe pruned word-bigram table is CC-BY-SA-4.0; see word-bigram-NOTICE.txt.\n')
    debian = stage / 'DEBIAN'
    debian.mkdir()
    size = sum(p.stat().st_size for p in stage.rglob('*') if p.is_file()) // 1024
    (debian / 'control').write_text(f'''Package: ibus-zhizi
Version: {package_version}
Architecture: amd64
Maintainer: Zhizi Project
Section: utils
Priority: optional
Installed-Size: {size}
Depends: python3 (>= 3.10), python3-gi, gir1.2-ibus-1.0, gir1.2-gtk-3.0, ibus (>= 1.5.26), libc6 (>= 2.38), libstdc++6, libgcc-s1
Description: Zhizi local Chinese input method for IBus
 Native pinyin, local learning, context ranking and optional sentence generation.
Homepage: https://zhizi.dabblecove.com
''')
    hashes = []
    for p in sorted(stage.rglob('*')):
        if p.is_file() and not p.is_symlink() and 'DEBIAN' not in p.relative_to(stage).parts:
            hashes.append(hashlib.sha256(p.read_bytes()).hexdigest() + '  ' + str(p.relative_to(stage)))
    (documentation / 'PAYLOAD-SHA256SUMS').write_text('\n'.join(hashes) + '\n')
    archive = output / f'ibus-zhizi_{package_version}_amd64.deb'
    subprocess.run(['dpkg-deb', '--root-owner-group', '-Zxz', '-z6', '--build', str(stage), str(archive)], check=True)
    record = dict(source, bytes=archive.stat().st_size, sha256=hashlib.sha256(archive.read_bytes()).hexdigest(),
                  payloadFiles=len(hashes), filename=archive.name)
    (output / (archive.name + '.json')).write_text(json.dumps(record, indent=2) + '\n')
    print(json.dumps(record))


if __name__ == '__main__':
    main()
