#!/usr/bin/env python3
"""R1 controls probe on a fresh temporary Story Tavern session.
Uses command endpoints only after the successful turn readback; no production data deletion.
"""
import json,os,sys,urllib.request,urllib.error
BASE=os.environ.get('KALEIDO_BASE','http://127.0.0.1:18766').rstrip('/')
def req(m,p,t,b=None,timeout=60):
 d=json.dumps(b).encode() if b is not None else None;h={'Content-Type':'application/json','Authorization':'Bearer '+t,'X-Mobile-Token':t};r=urllib.request.Request(BASE+p,data=d,headers=h,method=m)
 try:
  with urllib.request.urlopen(r,timeout=timeout) as x:
   z=x.read().decode();
   try:z=json.loads(z)
   except:pass
   return x.status,z
 except urllib.error.HTTPError as e:
  z=e.read().decode();
  try:z=json.loads(z)
  except:pass
  return e.code,z
 except Exception as e:return -1,str(e)
def main():
 env={};fp=os.path.join(os.path.dirname(__file__),'..','.env')
 for l in open(fp,encoding='utf8'):
  if '=' in l and not l.lstrip().startswith('#'):k,v=l.strip().split('=',1);env[k]=v
 st,x=req('POST','/api/v1/auth/login','',{'username':env.get('KALEIDO_ADMIN_USER'),'password':env.get('KALEIDO_ADMIN_PASSWORD')});
 if st!=200:print('R1_CTRL_LOGIN_FAIL',st,x);return 1
 t=x['token'];st,x=req('POST','/api/v1/story-tavern/sessions',t,{'packId':'demo-rain-alley','playable':'P1','playMode':'free','userTier':'standard','adultConfirmed':True});
 # session endpoint needs auth; helper above intentionally receives token
 if st!=200: print('R1_CTRL_SESSION_FAIL',st,x);return 2
 sid=x['sessionId'];st,a=req('POST',f'/api/v1/story-tavern/sessions/{sid}/turn',t,{'message':'请用一句话描述场景。','quality':'fast'},45);print('R1_CTRL_TURN',st,a if st!=200 else {'runId':a.get('runId')})
 if st!=200:return 3
 rid=a['runId'];st,b=req('POST',f'/api/v1/story-tavern/sessions/{sid}/stop',t,{'runId':rid});print('R1_CTRL_STOP',st,b)
 st,c=req('GET',f'/api/v1/story-tavern/sessions/{sid}',t);print('R1_CTRL_READBACK',st,{'activeRunId':c.get('activeRunId') if isinstance(c,dict) else None,'messages':len(c.get('messages',[])) if isinstance(c,dict) else None})
 return 0 if st==200 and isinstance(b,dict) and b.get('ok') and (not c.get('activeRunId')) else 4
if __name__=='__main__':sys.exit(main())
