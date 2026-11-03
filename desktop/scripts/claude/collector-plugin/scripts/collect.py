#!/usr/bin/env python3
"""Stop hook: metadata-only delivery; no modifications to Claude or its transcript."""
import datetime, fcntl, hashlib, hmac, json, os, pathlib, re, sys, tempfile, time, urllib.request, uuid
from urllib.parse import urlsplit
ID = re.compile(r'^[A-Za-z0-9_-]{1,160}$')
FIELDS = {'input_tokens':'input_tokens','output_tokens':'output_tokens','cache_read_input_tokens':'cache_read_tokens','cache_creation_input_tokens':'cache_write_tokens'}
def extract(payload, matching_key=None):
    sid=payload.get('session_id'); path=payload.get('transcript_path')
    if not isinstance(sid,str) or not ID.fullmatch(sid) or not isinstance(path,str): return []
    with open(path,'rb') as f:
        f.seek(0,2); size=f.tell(); f.seek(max(0,size-8*1024*1024)); data=f.read(8*1024*1024)
    rows={}; user_uuid=None
    for line in data.splitlines():
        try: e=json.loads(line)
        except (ValueError,UnicodeDecodeError): continue
        if not isinstance(e,dict): continue
        m=e.get('message')
        def valid_uuid(value):
            try:return str(uuid.UUID(value)) if isinstance(value,str) else None
            except ValueError:return None
        if isinstance(m,dict) and m.get('role')=='user':
            content=m.get('content')
            if isinstance(content,str) or (isinstance(content,list) and any(isinstance(b,dict) and b.get('type')=='text' for b in content)):
                user_uuid=valid_uuid(e.get('uuid'))
        if not isinstance(m,dict) or m.get('role')!='assistant' or not isinstance(m.get('stop_reason'),str): continue
        mid=m.get('id');model=m.get('model');u=m.get('usage')
        if not all(isinstance(v,str) and ID.fullmatch(v) for v in (mid,model)) or not model.startswith('claude-') or not isinstance(u,dict):continue
        if not all(type(u.get(k)) is int and 0<=u[k]<=100000000 for k in ('input_tokens','output_tokens')):continue
        # Reconstructed history can contain a placeholder model with zero usage.
        # Zero-only rows provide no evidence of a measured model invocation.
        if not any(type(u.get(k)) is int and u[k]>0 for k in FIELDS):continue
        stamp=e.get('timestamp')
        try: ts=int(datetime.datetime.fromisoformat(stamp.replace('Z','+00:00')).timestamp()*1000)
        except (AttributeError,ValueError,TypeError): continue
        r={'session_id':sid,'response_id':mid,'model':model,'timestamp_ms':ts}
        r.update(message_uuid=valid_uuid(e.get('uuid')),parent_message_uuid=valid_uuid(e.get('parentUuid')),user_message_uuid=user_uuid)
        content=m.get('content')
        body=''.join(b.get('text','') for b in content if isinstance(b,dict) and b.get('type')=='text' and isinstance(b.get('text'),str)) if isinstance(content,list) else ''
        r['response_fingerprint']=hmac.new(matching_key,b'kikimimi-response-v1\0'+body.replace('\r\n','\n').encode(),hashlib.sha256).hexdigest() if body and matching_key else None
        for k,v in FIELDS.items():
            n=u.get(k);r[v]=n if type(n) is int and 0<=n<=100000000 else None
        rows[mid]=r
    return list(rows.values())
class NoRedirect(urllib.request.HTTPRedirectHandler):
    # Never forward a credential to a redirected destination.
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None

def atomic_json(path, value):
    fd, name = tempfile.mkstemp(dir=path.parent)
    try:
        with os.fdopen(fd, 'w') as f:
            json.dump(value, f); f.flush(); os.fsync(f.fileno())
        os.replace(name, path)
    finally:
        if os.path.exists(name): os.unlink(name)

def main():
    payload=json.loads(sys.stdin.read(1048576) or '{}')
    root=pathlib.Path(__file__).resolve().parents[1]
    cfg=json.loads((root/'receiver.json').read_text())
    url=cfg['url']; token=cfg['token']
    u=urlsplit(url)
    if u.scheme!='https' or not u.hostname or u.username or u.password or u.query or u.fragment or u.path!='/v1/claude-plugin' or len(token)!=64:return
    expiry=cfg.get('expires_at')
    if expiry is not None and (type(expiry) is not int or expiry<=time.time()):raise RuntimeError('expired')
    state=pathlib.Path.home()/'.kikimimi-plugin';state.mkdir(mode=0o700,exist_ok=True)
    # Replaying completed responses is safe: receiver identities are stable across restarts.
    scope=hashlib.sha256((url+':matching-v1:'+str(payload.get('session_id'))).encode()).hexdigest()
    lock=open(state/(scope+'.lock'),'a')
    os.chmod(lock.name,0o600)
    try: fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
    except BlockingIOError:return
    checkpoint=state/(scope+'.json')
    try: sent=set(json.loads(checkpoint.read_text()))
    except (OSError,ValueError): sent=set()
    rows=[r for r in extract(payload,hashlib.sha256(token.encode()).hexdigest().encode()) if r['response_id'] not in sent]
    opener=urllib.request.build_opener(NoRedirect())
    deadline=time.monotonic()+7
    for start in range(0,len(rows),32):
        if time.monotonic()>=deadline:break
        body=json.dumps({'version':1,'responses':rows[start:start+32]}).encode()
        req=urllib.request.Request(url,data=body,headers={'Authorization':'Bearer '+token,'Content-Type':'application/json','User-Agent':'kikimimi-collector/0.4.1'})
        with opener.open(req,timeout=min(3,max(0.1,deadline-time.monotonic()))) as response:
            if response.status!=200:raise RuntimeError('receiver rejected batch')
        sent.update(r['response_id'] for r in rows[start:start+32])
        atomic_json(checkpoint,sorted(sent)[-20000:])
    atomic_json(state/'status.json',{'last_success':int(time.time()),'pending':sum(r['response_id'] not in sent for r in rows)})
if __name__=='__main__':
    try:main()
    except Exception as error:
        # Store only the exception class, never messages/URLs/headers or transcript data.
        try:
            state=pathlib.Path.home()/'.kikimimi-plugin';state.mkdir(mode=0o700,exist_ok=True)
            atomic_json(state/'status.json',{'last_failure':int(time.time()),'error':type(error).__name__})
        except Exception: pass
