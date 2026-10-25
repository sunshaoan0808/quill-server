#!/usr/bin/env python3
"""R1 control probe: reroll after a completed checkpoint and continue with fresh readback."""
import json,os,sys,urllib.request,urllib.error,time
BASE=os.environ.get('KALEIDO_BASE','http://127.0.0.1:18766').rstrip('/')
def req(m,p,t,b=None,timeout=180,stream=False):
 d=json.dumps(b).encode() if b is not None else None;h={'Authorization':'Bearer '+t,'X-Mobile-Token':t};
 if b is not None:h['Content-Type']='application/json'
 if stream:h['Accept']='text/event-stream'
 r=urllib.request.Request(BASE+p,data=d,headers=h,method=m)
 try:
  x=urllib.request.urlopen(r,timeout=timeout)
  if stream:return x
  z=x.read().decode('utf8','replace')
  try:z=json.loads(z)
  except:pass
  return x.status,z
 except urllib.error.HTTPError as e:
  z=e.read().decode('utf8','replace')
  try:z=json.loads(z)
  except:pass
  return e.code,z
 except Exception as e:return -1,{'error':str(e)}
def stream(t,sid,rid):
 x=req('GET',f'/api/v1/story-tavern/sessions/{sid}/stream?runId={rid}',t,timeout=180,stream=True);ev=[]
 if isinstance(x,tuple):return ev
 with x:
  while True:
   q=x.readline()
   if not q:break
   q=q.decode('utf8','replace').strip()
   if q.startswith('data:'):
    try:
     o=json.loads(q[5:].strip());ev.append(o.get('type'))
     if o.get('type') in ('done','error'):break
    except:pass
 return ev
def main():
 env={}
 for l in open(os.path.join(os.path.dirname(__file__),'..','.env'),encoding='utf8'):
  if '=' in l and not l.lstrip().startswith('#'):k,v=l.strip().split('=',1);env[k]=v
 st,x=req('POST','/api/v1/auth/login','',{'username':env.get('KALEIDO_ADMIN_USER'),'password':env.get('KALEIDO_ADMIN_PASSWORD')})
 if st!=200:return print('R1_RC_LOGIN_FAIL',st,x) or 1
 t=x['token'];st,x=req('POST','/api/v1/story-tavern/sessions',t,{'packId':'demo-rain-alley','playable':'P1','playMode':'free','userTier':'standard','adultConfirmed':True})
 if st!=200:return print('R1_RC_SESSION_FAIL',st,x) or 2
 sid=x['sessionId']
 def turn(msg):
  st,z=req('POST',f'/api/v1/story-tavern/sessions/{sid}/turn',t,{'message':msg,'quality':'fast'},45)
  if st!=200:return st,z,[]
  return st,z,stream(t,sid,z['runId'])
 st,z,ev=turn('请用一句话描述场景。');print('R1_RC_BASE',st,ev[-3:])
 time.sleep(1)
 st,back=req('GET',f'/api/v1/story-tavern/sessions/{sid}',t);print('R1_RC_AFTER_BASE',st,{'turn':back.get('turn'),'checkpoints':len(back.get('checkpoints',[])),'revision':back.get('revision')})
 st,r=req('POST',f'/api/v1/story-tavern/sessions/{sid}/reroll',t,{});print('R1_REROLL',st,r)
 st,back=req('GET',f'/api/v1/story-tavern/sessions/{sid}',t);print('R1_REROLL_READBACK',st,{'turn':back.get('turn'),'messages':len(back.get('messages',[]))})
 st,z,ev2=turn('请继续一句。');print('R1_CONTINUE_EXPLICIT',st,ev2[-3:] if ev2 else z)
 st,back=req('GET',f'/api/v1/story-tavern/sessions/{sid}',t);print('R1_FINAL_READBACK',st,{'turn':back.get('turn'),'messages':len(back.get('messages',[])),'activeRunId':back.get('activeRunId')})
 # Reroll is the server's regenerate endpoint; empty-message continuation is separately recorded as a known revision conflict if it persists.
 return 0 if st==200 and ev and ev[-1]=='done' and ev2 and ev2[-1]=='done' and isinstance(r,dict) and r.get('ok') else 3
if __name__=='__main__':sys.exit(main())