#!/usr/bin/env python3
"""Assemble a standalone SDK from the existing reviewed public-source policy.

No publishing, app installation, network fetch or new Git checkout is performed.
Product-internal tests stay in the product repository; the SDK retains its
public integration tests and owned-result unit tests, with release code intact.
"""
import argparse
from datetime import datetime, timezone
import hashlib
import importlib.util
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def load_module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--src', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    source = args.src.resolve(strict=True)
    out = args.out.absolute()
    if out.exists() or out.is_symlink():
        raise ValueError('output must not already exist')
    source_commit = subprocess.check_output(['git', '-C', str(source), 'rev-parse', 'HEAD'], text=True).strip()
    # Other chats may own unrelated documentation changes. The exported SDK
    # itself must have a committed source identity.
    dirty = subprocess.check_output(['git', '-C', str(source), 'status', '--porcelain', '--',
        'native/core', 'scripts/tools/export_zhixu_sdk.py'], text=True)
    if dirty.strip():
        raise ValueError('commit SDK source before export')
    policy = source / 'scripts/tools/public_export.json'
    helper = load_module('sdk_public_repository_helpers', source / 'scripts/tools/make_public_repo.py')
    with tempfile.TemporaryDirectory(prefix='zhixu-sdk-public-') as temporary:
        stage = Path(temporary)
        public = stage / 'public'
        report = stage / 'public-export.json'
        with (stage / 'public-export.log').open('w') as log:
            result = subprocess.run([sys.executable, str(source / 'scripts/tools/public_export.py'),
                '--src', str(source), '--out', str(public), '--config', str(policy), '--report', str(report)],
                stdout=log, stderr=subprocess.STDOUT)
        if result.returncode:
            raise ValueError('public-source policy rejected export')
        public_report = json.loads(report.read_text())
        if public_report['blocker']:
            raise ValueError('public export contains blockers')
        helper.regenerate_derived(source, public)
        crate = public / 'native/core/agent-ime-core'
        shutil.copytree(crate, out)
        shutil.copy2(public / 'native/core/Cargo.lock', out / 'Cargo.lock')
        shutil.copy2(public / 'LICENSE', out / 'LICENSE')
        shutil.copy2(public / 'THIRD_PARTY_NOTICES.md', out / 'THIRD_PARTY_NOTICES.md')
        shutil.copytree(public / 'third_party/licenses', out / 'licenses/third-party')
        rust_licenses = source / '.build-deps/rust/1.94.1/licenses'
        for name in ['LICENSE-APACHE', 'LICENSE-MIT', 'COPYRIGHT-library.html']:
            dest = out / 'licenses/rust-standard-library' / name
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(rust_licenses / name, dest)
        transforms = []
        assets = {}
        unavailable = []
        include = re.compile(r'include_(str|bytes)!\(\s*"([^"\n]+)"\s*\)')
        annotation = re.compile(r'#\[(cfg|cfg_attr)\((.*?)\)\]', re.S)
        cfg_token = re.compile(r'"(?:[^"\\]|\\.)*"|\btest\b')
        for original in sorted((crate / 'src').rglob('*.rs')):
            relative = original.relative_to(crate)
            destination = out / relative
            text = original.read_text()
            initial = text
            # A release build evaluates test=false. Substitute the same false
            # condition for product-private test branches in the distribution.
            # This does not disable the SDK's owned-result or public tests.
            if relative.name not in ('sdk.rs', 'sdk_owned.rs'):
                def condition(match):
                    body = cfg_token.sub(lambda token: 'any()' if token.group() == 'test' else token.group(), match.group(2))
                    return '#[' + match.group(1) + '(' + body + ')]'
                text = re.sub(r'(?m)^([ \t]*)#\[test\]', r'\1#[cfg(any())]\n\1#[test]', text)
                text = annotation.sub(condition, text)
                text = text.replace('cfg!(test)', 'cfg!(any())')
            def asset(match):
                path = (original.parent / match.group(2)).resolve()
                if path.is_relative_to(crate.resolve()):
                    return match.group()
                if not path.is_relative_to(public.resolve()) or not path.is_file():
                    unavailable.append({'source': str(relative), 'include': match.group(2)})
                    return match.group()  # disabled internal tests; live misses fail the build
                name = path.relative_to(public.resolve()).as_posix()
                packaged = 'resources/sdk-data/' + name
                target = out / packaged
                target.parent.mkdir(parents=True, exist_ok=True)
                if not target.exists():
                    shutil.copy2(path, target)
                assets[name] = {'packaged': packaged, 'sha256': digest(target)}
                return 'include_' + match.group(1) + '!(concat!(env!("CARGO_MANIFEST_DIR"), "/' + packaged + '"))'
            text = include.sub(asset, text)
            if text != initial:
                destination.write_text(text)
                transforms.append({'file': str(relative), 'sourceSHA256': digest(original), 'exportSHA256': digest(destination)})
        # Only SDK tests are part of the independent distribution. Product
        # fixture branches above are disabled; do not redistribute their data.
        for internal in [out / 'tests/fixtures', out / 'testdata']:
            if internal.exists():
                shutil.rmtree(internal)
        dictionary = out / 'dictionary'
        shutil.copytree(public / 'native/resources/Rime', dictionary)
        pinned = source / '.build-deps/rime-pinyin-simp/0c6861ef7420ee780270ca6d993d18d4101049d0'
        shutil.copy2(pinned / 'pinyin_simp.dict.yaml', dictionary / 'pinyin_simp.dict.yaml')
        for name in ['LICENSE', 'AUTHORS', 'LOCKED_VERSION']:
            target = out / 'licenses/rime-pinyin-simp' / name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(pinned / name, target)
        for path in out.rglob('*'):
            if path.is_symlink():
                raise ValueError('SDK contains a symlink')
        findings = helper.scan(out, policy)
        registry = json.loads((source / 'docs/current/licenses/NATIVE_LICENSE_ASSETS.json').read_text())
        license_paths = {
            'licenses/rust-standard-library/COPYRIGHT-library.html': '.build-deps/rust/1.94.1/licenses/COPYRIGHT-library.html',
            'licenses/third-party/pypinyin-MIT.txt': 'third_party/licenses/pypinyin-MIT.txt',
            'licenses/rime-pinyin-simp/AUTHORS': '.build-deps/rime-pinyin-simp/0c6861ef7420ee780270ca6d993d18d4101049d0/AUTHORS',
        }
        verified_licenses = {}
        for packaged, original in license_paths.items():
            asset = next((a for a in registry['assets'] if a.get('repositoryPath') == original), None)
            if not asset or digest(out / packaged) != asset['sha256']:
                raise ValueError('upstream license differs from locked attribution registry')
            verified_licenses[packaged] = asset['sha256']
        reviewed = [hit for hit in findings['warning'] if hit['rule'] == 'email_address' and hit['file'] in verified_licenses]
        unreviewed = [hit for hit in findings['warning'] if hit not in reviewed]
        if findings['blocker'] or unreviewed:
            raise ValueError('SDK has a source blocker or unreviewed warning')
        findings['reviewedPublicAttributionContacts'] = reviewed
        findings['verifiedAttributionFileSHA256'] = verified_licenses
        findings['unreviewedWarnings'] = unreviewed
        metadata = {
            'formatVersion': 1, 'component': 'zhixu-sdk', 'version': '0.3.1', 'abi': 6,
            'sourceCommit': source_commit, 'createdAtUTC': datetime.now(timezone.utc).isoformat(),
            'publicPolicySHA256': digest(policy), 'publicExportReportSHA256': digest(report),
            'publicPolicyApplied': True, 'professionalLayersStubbedAndDerivedTablesRegenerated': True,
            'productionNeuralModelsBundled': False, 'clientAppSourcesBundled': False,
            'compileTimeAssetsRelocated': assets, 'sourceTransforms': transforms,
            'unavailableInternalTestAssetsNotCopied': unavailable,
            'testScope': 'SDK public integration and owned-result tests only; private product tests remain in product repository; release cfg behavior preserved',
            'scan': findings, 'basePinyinSHA256': digest(dictionary / 'pinyin_simp.dict.yaml'),
            'files': {p.relative_to(out).as_posix(): digest(p) for p in sorted(out.rglob('*')) if p.is_file()},
            'publishing': 'local artifact only; not uploaded to a public registry or repository',
        }
        (out / 'SDK-MANIFEST.json').write_text(json.dumps(metadata, ensure_ascii=False, indent=2) + '\n')
        (out / 'SHA256SUMS').write_text(''.join(value + '  ' + key + '\n' for key, value in metadata['files'].items()))
        print(json.dumps({'version': metadata['version'], 'abi': 6, 'files': len(metadata['files']),
            'assets': len(assets), 'sourceCommit': source_commit, 'noBlockersOrUnreviewedWarnings': True}))
    return 0


if __name__ == '__main__':
    sys.exit(main())
