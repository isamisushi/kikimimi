#!/usr/bin/env python3
"""Prepare private tunnel config and LaunchAgent, without changing DNS or starting it.

Requires an already-created named tunnel and its tunnel-specific credentials.
Never uses/stores the Cloudflare account certificate in the service arguments.
"""
import argparse, json, os, pathlib, plistlib, re

def exclusive(path, data):
    fd=os.open(path,os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600)
    with os.fdopen(fd,'wb') as f:
        f.write(data);f.flush();os.fsync(f.fileno())

def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--hostname',required=True)
    p.add_argument('--credentials',type=pathlib.Path,required=True)
    p.add_argument('--output-dir',type=pathlib.Path,required=True)
    p.add_argument('--cloudflared',type=pathlib.Path,default=pathlib.Path('/opt/homebrew/bin/cloudflared'))
    a=p.parse_args()
    if not re.fullmatch(r'[a-z0-9](?:[a-z0-9.-]{0,251}[a-z0-9])?',a.hostname) or '..' in a.hostname:p.error('Invalid hostname')
    c=json.loads(a.credentials.read_text());tid=c.get('TunnelID','')
    if not re.fullmatch(r'[0-9a-f-]{36}',tid):p.error('Invalid tunnel ID')
    a.output_dir.mkdir(mode=0o700,parents=True,exist_ok=True)
    root=a.output_dir.resolve();cred=root/'tunnel-credentials.json';config=root/'config.json'
    exclusive(cred,json.dumps(c).encode())
    exclusive(config,json.dumps({'tunnel':tid,'credentials-file':str(cred),'metrics':'127.0.0.1:20242','ingress':[{'hostname':a.hostname,'path':'^/v1/claude-plugin$','service':'http://127.0.0.1:43192'},{'service':'http_status:404'}]},indent=2).encode())
    job={'Label':'dev.kikimimi.claude-tunnel','ProgramArguments':[str(a.cloudflared.resolve()),'tunnel','--no-autoupdate','--config',str(config),'run',tid],'RunAtLoad':True,'KeepAlive':True,'ThrottleInterval':30,'ProcessType':'Background','StandardOutPath':str(root/'tunnel.log'),'StandardErrorPath':str(root/'tunnel-error.log')}
    exclusive(root/'dev.kikimimi.claude-tunnel.plist',plistlib.dumps(job))
    print('Private tunnel config and dedicated LaunchAgent prepared. No DNS or running services changed.')
if __name__=='__main__':main()
