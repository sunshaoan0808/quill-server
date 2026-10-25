#!/usr/bin/env python3
"""R1 final reroll check: completed first turn SSE, then reroll/readback."""
import json, os, sys, time, urllib.request, urllib.error
BASE = os.environ.get('KALEIDO_BASE', 'http://127.0.0.1:18766').rstrip('/')

def req(method, path, token, body=None, timeout=180):
    data = json.dumps(body).encode() if body is not None else None
    headers = {'Authorization': 'Bearer ' + token, 'X-Mobile-Token': token}
    if body is not None: headers['Content-Type'] = 'application/json'
    r = urllib.request.Request(BASE + path, data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(r, timeout=timeout) as x:
            raw = x.read().decode()
            try: raw = json.loads(raw)
            except Exception: pass
            return x.status, raw
    except urllib.error.HTTPError as e:
        raw = e.read().decode()
        try: raw = json.loads(raw)
        except Exception: pass
        return e.code, raw
    except Exception as e:
        return -1, {'error': str(e)}

def read_sse(token, sid, rid):
    h = {'Authorization': 'Bearer ' + token, 'X-Mobile-Token': token, 'Accept': 'text/event-stream'}
    r = urllib.request.Request(BASE + f'/api/v1/story-tavern/sessions/{sid}/stream?runId={rid}', headers=h)
    events = []
    with urllib.request.urlopen(r, timeout=240) as x:
        while True:
            line = x.readline()
            if not line: break
            text = line.decode('utf8', 'replace').strip()
            if not text or not text.startswith('data:'): continue
            try:
                obj = json.loads(text[5:].strip())
                events.append(obj.get('type'))
                if obj.get('type') in ('done', 'error'): break
            except Exception: pass
    return events

def main():
    env = {}
    for line in open('/root/museai-server/.env', encoding='utf8'):
        if '=' in line and not line.lstrip().startswith('#'):
            k, v = line.strip().split('=', 1); env[k] = v
    st, login = req('POST', '/api/v1/auth/login', '', {'username': env.get('KALEIDO_ADMIN_USER'), 'password': env.get('KALEIDO_ADMIN_PASSWORD')})
    if st != 200: print('LOGIN', st, login); return 1
    token = login['token']
    st, session = req('POST', '/api/v1/story-tavern/sessions', token, {'packId':'demo-rain-alley','playable':'P1','playMode':'free','userTier':'standard','adultConfirmed':True})
    if st != 200: print('SESSION', st, session); return 2
    sid = session['sessionId']
    st, turn = req('POST', f'/api/v1/story-tavern/sessions/{sid}/turn', token, {'message':'请用一句话描述当前场景。','quality':'fast'}, 45)
    print('TURN', st, turn if st != 200 else {'runId': turn.get('runId')})
    if st != 200: return 3
    rid = turn['runId']
    events = read_sse(token, sid, rid)
    print('SSE', {'events': events[-8:], 'done': bool(events and events[-1] == 'done')})
    st, base = req('GET', f'/api/v1/story-tavern/sessions/{sid}', token)
    print('BASE_READBACK', st, {'turn':base.get('turn') if isinstance(base,dict) else None, 'messages':len(base.get('messages',[])) if isinstance(base,dict) else None, 'checkpoints':len(base.get('checkpoints',[])) if isinstance(base,dict) else None})
    st, reroll = req('POST', f'/api/v1/story-tavern/sessions/{sid}/reroll', token, {})
    print('REROLL', st, reroll)
    st, final = req('GET', f'/api/v1/story-tavern/sessions/{sid}', token)
    print('REROLL_READBACK', st, {'turn':final.get('turn') if isinstance(final,dict) else None, 'messages':len(final.get('messages',[])) if isinstance(final,dict) else None, 'checkpoints':len(final.get('checkpoints',[])) if isinstance(final,dict) else None, 'activeRunId':final.get('activeRunId') if isinstance(final,dict) else None})
    return 0 if events and events[-1] == 'done' and st == 200 and isinstance(reroll,dict) and reroll.get('ok') else 4

if __name__ == '__main__': sys.exit(main())
