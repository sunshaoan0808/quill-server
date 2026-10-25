#!/usr/bin/env python3
"""R1 live probe: Story Tavern create -> turn -> SSE -> session readback.
No destructive operations; uses a fresh temporary session and a minimal prompt.
"""
import json, os, sys, time, urllib.error, urllib.request
BASE=os.environ.get('KALEIDO_BASE','http://127.0.0.1:18766').rstrip('/')

def req(method,path,token=None,body=None,timeout=30,accept=None):
    data=json.dumps(body).encode() if body is not None else None
    h={'Content-Type':'application/json'} if body is not None else {}
    if token: h.update(Authorization='Bearer '+token, **{'X-Mobile-Token':token})
    if accept: h['Accept']=accept
    r=urllib.request.Request(BASE+path,data=data,headers=h,method=method)
    try:
        with urllib.request.urlopen(r,timeout=timeout) as x:
            raw=x.read().decode('utf-8','replace')
            try:return x.status,json.loads(raw)
            except:return x.status,raw
    except urllib.error.HTTPError as e:
        raw=e.read().decode('utf-8','replace')
        try: body=json.loads(raw)
        except: body=raw[:500]
        return e.code,body
    except Exception as e:return -1,{'error':str(e)}

def main():
    env={}
    ep=os.path.join(os.path.dirname(__file__),'..','.env')
    if os.path.exists(ep):
        for line in open(ep,encoding='utf8'):
            if '=' in line and not line.lstrip().startswith('#'):
                k,v=line.strip().split('=',1);env[k]=v
    st,login=req('POST','/api/v1/auth/login',body={'username':env.get('KALEIDO_ADMIN_USER'),'password':env.get('KALEIDO_ADMIN_PASSWORD')})
    if st!=200: print('R1_LOGIN_FAIL',st,login);return 1
    token=login.get('token'); print('R1_LOGIN_OK')
    # Existing demo pack is read-only fixture; create only a session.
    st,packs=req('GET','/api/v1/story-tavern/packs',token=token)
    pack='demo-rain-alley'
    if isinstance(packs,dict):
        rows=packs.get('packs') or packs.get('items') or []
        if rows: pack=rows[0].get('id') or rows[0].get('packId') or pack
    st,s=req('POST','/api/v1/story-tavern/sessions',token=token,body={'packId':pack,'playable':'P1','playMode':'free','userTier':'standard','adultConfirmed':True})
    print('R1_SESSION',st,s if st!=200 else {'sessionId':s.get('sessionId')})
    if st!=200 or not s.get('sessionId'): return 1
    sid=s['sessionId']
    st,t=req('POST',f'/api/v1/story-tavern/sessions/{sid}/turn',token=token,body={'message':'请用一句话描述当前场景。','quality':'fast'},timeout=45)
    print('R1_TURN',st,t if st!=200 else {'runId':t.get('runId')})
    if st!=200 or not t.get('runId'): return 2
    rid=t['runId']; h={'Authorization':'Bearer '+token,'X-Mobile-Token':token,'Accept':'text/event-stream'}
    r=urllib.request.Request(BASE+f'/api/v1/story-tavern/sessions/{sid}/stream?runId={rid}',headers=h)
    events=[]; raw=''; done=False
    try:
        with urllib.request.urlopen(r,timeout=180) as x:
            while True:
                line=x.readline()
                if not line: break
                text=line.decode('utf8','replace').strip()
                if not text or text.startswith(':'): continue
                if text.startswith('data:'): text=text[5:].strip()
                raw+=text+'\n'
                try:
                    o=json.loads(text);events.append(o.get('type'))
                    if o.get('type') in ('done','error'): done=True;break
                except: pass
    except Exception as e: print('R1_SSE_EXCEPTION',repr(e))
    print('R1_SSE',{'events':events,'done':done,'raw_bytes':len(raw)})
    st,back=req('GET',f'/api/v1/story-tavern/sessions/{sid}',token=token)
    msgs=(back.get('messages') if isinstance(back,dict) else []) or []
    print('R1_READBACK',st,{'messages':len(msgs),'activeRunId':back.get('activeRunId') if isinstance(back,dict) else None})
    return 0 if done and st==200 and len(msgs)>=2 else 3
if __name__=='__main__':sys.exit(main())
