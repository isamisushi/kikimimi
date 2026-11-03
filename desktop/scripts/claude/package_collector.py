#!/usr/bin/env python3
"""Package the collector for an explicitly approved HTTPS receiver.

The archive contains a receiver credential. Keep it private, never commit/share it,
remove it after upload, and rotate the key after a temporary test.
"""
import argparse, json, os
from pathlib import Path
from urllib.parse import urlsplit
from zipfile import ZipFile, ZIP_DEFLATED

def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--url',required=True)
    p.add_argument('--config',type=Path,required=True)
    p.add_argument('--output',type=Path,required=True)
    a=p.parse_args();u=urlsplit(a.url)
    if u.scheme!='https' or not u.hostname or u.username or u.password or u.query or u.fragment or u.path!='/v1/claude-plugin':
        p.error('Use the approved HTTPS URL ending in /v1/claude-plugin, without credentials/query/fragment')
    c=json.loads(a.config.read_text())
    if c.get('url') != a.url: p.error('Packaging URL must match the exported URL')
    if not isinstance(c.get('token'),str) or len(c['token'])<32:p.error('Invalid receiver configuration')
    fd=os.open(a.output,os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600)
    root=Path(__file__).parent/'collector-plugin'
    with os.fdopen(fd,'wb') as out, ZipFile(out,'w',ZIP_DEFLATED) as z:
        for f in root.rglob('*'):
            if f.is_file() and '__pycache__' not in f.parts:z.write(f,str(f.relative_to(root)))
        z.writestr('receiver.json',json.dumps({'url':a.url,'token':c['token'],'expires_at':c['expires_at']}))
    print('Private collector archive created; credential not displayed.')
if __name__=='__main__':main()
