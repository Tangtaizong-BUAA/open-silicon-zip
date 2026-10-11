#!/usr/bin/env python3
"""Check literal UI/code keys and format arguments against shipped tables."""
import pathlib,re,sys
root=pathlib.Path(__file__).resolve().parents[1]
pattern=r'"((?:[^"\\]|\\.)*)"\s*=\s*"((?:[^"\\]|\\.)*)";'
keys=set()
code_keys=set()
for p in (root/'app/Sources').rglob('*.swift'):
    code_keys.update(re.findall(r'L10n\.[tf]\("((?:[^"\\]|\\.)*)"',p.read_text()))
for p in (root/'app/Sources/FastZipApp').glob('*.swift'):
    keys.update(re.findall(r'(?:Text|Label|Button|Picker|Toggle|Section|Menu|Window|WindowGroup|TableColumn|help|navigationTitle|accessibilityLabel)\("((?:[^"\\]|\\.)*)"',p.read_text()))
keys={k for k in keys if '\\(' not in k and k and re.search('[A-Z][a-z]',k) and k!='Open Silicon Zip'}
keys |= code_keys
errors=[]
for lang in ('en','zh-Hans','zh-Hant'):
    p=root/f'app/Sources/FastZipKit/Resources/{lang}.lproj/Localizable.strings'
    pairs=re.findall(pattern,p.read_text());table=dict(pairs)
    app_table=dict(re.findall(pattern,(root/f'app/Sources/FastZipApp/Resources/{lang}.lproj/Localizable.strings').read_text()))
    if app_table != table: errors.append(f'{lang}: app and kit translation tables differ')
    if len(table)!=len(pairs):errors.append(f'{lang}: duplicate translation keys')
    for key in sorted(keys-table.keys()):errors.append(f'{lang}: missing {key}')
    for key,value in pairs:
        args=lambda s: sorted(re.findall(r'%(?:\d+\$)?(?:@|lld|llu|ld|lu|d|u|f)',s))
        if args(key)!=args(value):errors.append(f'{lang}: format mismatch {key}')
print('\n'.join(errors) if errors else f'PASS: {len(keys)} literal interface keys, three language tables and format placeholders')
sys.exit(bool(errors))
