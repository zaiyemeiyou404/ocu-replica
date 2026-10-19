"""Live tests against ocu-replica, using only a disposable owned WinForms app.
All observed app values come from the fixture's independent state file.
"""
import argparse, base64, ctypes, json, os, pathlib, queue, re, subprocess, sys, threading, time, traceback
from PIL import Image

ROOT = pathlib.Path(__file__).resolve().parent
user32 = ctypes.WinDLL('user32', use_last_error=True)
user32.GetForegroundWindow.restype = ctypes.c_void_p
class Point(ctypes.Structure):
    _fields_ = [('x', ctypes.c_long), ('y', ctypes.c_long)]
class LastInput(ctypes.Structure):
    _fields_ = [('cbSize',ctypes.c_uint),('dwTime',ctypes.c_uint)]
def context():
    p = Point(); user32.GetCursorPos(ctypes.byref(p))
    last=LastInput(ctypes.sizeof(LastInput),0);user32.GetLastInputInfo(ctypes.byref(last))
    return {'foreground': user32.GetForegroundWindow(), 'cursor': [p.x,p.y],'last_input_tick':last.dwTime}
def read_state():
    for _ in range(30):
        try: return json.loads((ROOT/'fixture-state.json').read_text(encoding='utf-8-sig'))
        except (OSError, ValueError): time.sleep(.03)
    raise RuntimeError('fixture state unavailable')
def wait_state(predicate, timeout=3):
    end=time.monotonic()+timeout
    while time.monotonic()<end:
        state=read_state()
        if predicate(state):return state
        time.sleep(.04)
    raise AssertionError(state)
def command(op):
    before = read_state(); seq = before['command']+1
    (ROOT/'fixture-command.tmp').write_text(json.dumps({'id':seq,'op':op}), encoding='utf-8')
    os.replace(ROOT/'fixture-command.tmp',ROOT/'fixture-command.json')
    deadline=time.monotonic()+4
    while time.monotonic()<deadline:
        state=read_state()
        if state['command']==seq:
            if state['error']: raise RuntimeError(state['error'])
            time.sleep(.15); return read_state()
        time.sleep(.05)
    raise TimeoutError('fixture command '+op)

class Client:
    def __init__(self, exe, out, supervise=False, approved_apps=None):
        self.out=out; self.seq=0; self.q=queue.Queue(); self.calls=[]
        approved_apps=approved_apps or 'ocu-lab-fixture.exe,ocu-lab-wpf.exe'
        self.observed_apps=approved_apps.split(',')
        env=os.environ.copy();env.update(OCU_REPLICA_HOME=str(out/'helper-home'),OCU_PREAPPROVE=approved_apps,OCU_INPUT_MODE='background',OCU_ALLOW_POSTMESSAGE='0')
        self.err=(out/'helper-stderr.log').open('w',encoding='utf-8')
        cmd=[sys.executable,str(ROOT.parent/'runtime'/'supervisor.py'),'--exe',exe] if supervise else [exe]
        self.p=subprocess.Popen(cmd,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=self.err,text=True,encoding='utf-8',env=env,creationflags=subprocess.CREATE_NO_WINDOW)
        threading.Thread(target=self._read,daemon=True).start()
    def _read(self):
        for line in self.p.stdout:
            try: self.q.put(json.loads(line))
            except ValueError: self.q.put({'badLine':line})
    def rpc(self, method, params, timeout=12):
        self.seq+=1; ident=self.seq; start=time.monotonic()
        req={'jsonrpc':'2.0','id':ident,'method':method,'params':params}
        self.p.stdin.write(json.dumps(req,ensure_ascii=False)+'\n');self.p.stdin.flush()
        try: res=self.q.get(timeout=timeout)
        except queue.Empty: raise TimeoutError(f'helper did not answer {method} within {timeout}s')
        if res.get('id')!=ident: raise RuntimeError(f'response id mismatch: {res.get("id")} != {ident}')
        safe=json.loads(json.dumps(res))
        if method=='tools/call' and params.get('name')=='list_windows':
            for block in safe.get('result',{}).get('content',[]):
                if block.get('type')=='text':
                    windows=json.loads(block['text'])
                    block['text']=json.dumps([w for w in windows if w.get('app','').lower() in [a.lower() for a in self.observed_apps]],ensure_ascii=False)
        for i,block in enumerate(safe.get('result',{}).get('content',[])):
            if block.get('type')=='image':
                fn=f'call-{ident}-image-{i}.png';(self.out/fn).write_bytes(base64.b64decode(block['data']));block['data']='(saved '+fn+')'
        self.calls.append({'request':req,'response':safe,'elapsed_ms':round((time.monotonic()-start)*1000,1)})
        (self.out/'calls.json').write_text(json.dumps(self.calls,ensure_ascii=False,indent=2),encoding='utf-8')
        return res
    def tool(self,name,args=None,timeout=12,meta=None):
        params={'name':name,'arguments':args or {}}
        if meta: params['_meta']=meta
        res=self.rpc('tools/call',params,timeout)
        if 'error' in res: raise RuntimeError(str(res['error']))
        result=res['result'];text='\n'.join(x['text'] for x in result.get('content',[]) if x['type']=='text')
        if result.get('isError'): raise RuntimeError(text)
        return json.loads(text),result
    def close(self):
        if self.p.poll() is None:
            try:self.p.stdin.close();self.p.wait(timeout=2)
            except Exception:self.p.kill();self.p.wait()
        self.err.close()

def indices(tree):
    found={}
    for line in tree.splitlines():
        match=re.match(r'^(\d+)\s+(\S+)\s+"([^\"]+)"',line)
        if match and match[2]!='text': found[match[3]]=int(match[1])
    return found

def main():
    ap=argparse.ArgumentParser();ap.add_argument('--exe',default=str(ROOT.parent/'bin'/'ocu-native.exe'));ap.add_argument('--label',default='latest');ap.add_argument('--fixture',default='ocu-lab-fixture.exe');ap.add_argument('--supervise',action='store_true');args=ap.parse_args()
    out=ROOT/args.label;out.mkdir(exist_ok=True)
    results=[]; client=None; fixture=None; context_notes=[]
    def record(name,func):
        context_notes.clear()
        start=time.monotonic()
        try:
            detail=func();row={'test':name,'status':'PARTIAL' if context_notes else 'PASS','detail':detail}
        except Exception as exc:
            row={'test':name,'status':'FAIL','detail':str(exc)}
        row['elapsed_ms']=round((time.monotonic()-start)*1000,1);results.append(row)
        if context_notes:row['context_notes']=list(context_notes)
        print(json.dumps(row,ensure_ascii=False),flush=True)
        (out/'results.json').write_text(json.dumps(results,ensure_ascii=False,indent=2),encoding='utf-8')
    try:
        # Start only our uniquely named fixture; never discover or target personal apps.
        old=ROOT/'fixture-command.json'
        if old.exists():old.rename(ROOT/f'fixture-command-{time.time_ns()}.json')
        old=ROOT/'fixture-state.json'
        if old.exists():old.rename(ROOT/f'fixture-state-{time.time_ns()}.json')
        fixture=subprocess.Popen([str(ROOT/args.fixture),str(ROOT)],creationflags=subprocess.CREATE_NO_WINDOW)
        deadline=time.monotonic()+8
        while not (ROOT/'fixture-state.json').exists():
            if time.monotonic()>deadline:raise TimeoutError('fixture failed to open')
            time.sleep(.1)
        client=Client(args.exe,out,args.supervise)
        init=client.rpc('initialize',{'protocolVersion':'2025-03-26','capabilities':{},'clientInfo':{'name':'ocu-native-lab','version':'1'}})
        assert init.get('result',{}).get('serverInfo',{}).get('name')=='ocu-replica',init
        rows,_=client.tool('list_windows')
        matches=[w for w in rows if w.get('title')=='OCU Native Lab - disposable target']
        assert len(matches)==1,matches
        window=matches[0]
        assert window['id']==read_state()['hwnd'],'target handle mismatch'
        def snap(image=False):
            return client.tool('get_window_state',{'window':window,'include_screenshot':image,'include_text':True,'input_mode':'background'})
        def observation():
            payload,_=snap();return payload,indices(payload['accessibility']['tree'])
        def no_interference(before):
            after=context()
            if before['last_input_tick']!=after['last_input_tick']:
                context_notes.append({'verification':'inconclusive_due_to_concurrent_user_input','before':before,'after':after})
                return
            assert before==after,{'before':before,'after':after}
        def enum_test():
            payload,idx=observation()
            assert all(k in idx for k in ['FieldA','FieldB','CommitA','CommitB']),payload
            return {'window':window,'elements':idx}
        record('enumerate_and_uia',enum_test)
        def capture_test():
            command('cover'); before=context();payload,blocks=snap(True);no_interference(before)
            imgs=[b for b in blocks['content'] if b['type']=='image'];assert len(imgs)==1,'missing MCP image'
            fn=out/'occluded-target.png';fn.write_bytes(base64.b64decode(imgs[0]['data']))
            im=Image.open(fn).convert('RGB');pix=list(im.resize((100,100)).get_flattened_data())
            blue=sum(b>r*1.4 and g>r*1.3 for r,g,b in pix)/len(pix)
            magenta=sum(r>180 and b>180 and g<80 for r,g,b in pix)/len(pix)
            assert blue>.08 and magenta<.15,{'blue':blue,'magenta':magenta}
            diag,_=client.tool('diagnostic_state',{'window':window})
            return {'backend':payload.get('backend'),'image_size':im.size,'blue_fraction':blue,'magenta_fraction':magenta,'diagnostic':diag,'context':before}
        record('occluded_capture_no_focus_or_cursor_change',capture_test)
        def click_test():
            command('reset');_,idx=observation();before=context()
            payload,_=client.tool('click_element',{'window':window,'element_index':idx['CommitB'],'input_mode':'background'})
            state=wait_state(lambda s:s['bClicks']==1);no_interference(before)
            assert state['bClicks']==1 and state['aClicks']==0,state
            assert payload['backend'] in ['uia','win32_message'],payload
            return {'backend':payload['backend'],'clicks':[state['aClicks'],state['bClicks']]}
        record('background_click_with_independent_readback',click_test)
        def toggle_test():
            command('reset');command('cover');_,idx=observation();before=context()
            payload,_=client.tool('click_element',{'window':window,'element_index':idx['TestToggle'],'input_mode':'background'})
            state=wait_state(lambda s:s['toggle']);no_interference(before)
            return {'checked':state['toggle'],'backend':payload.get('backend')}
        record('background_checkbox_toggle_with_readback',toggle_test)
        def set_value_test():
            command('reset');command('cover');before=context();times=[]
            for i in range(20):
                _,idx=observation();text='后台中文 / 测试 '+str(i)+' / emoji 😀'
                start=time.monotonic();client.tool('set_value',{'window':window,'element_index':idx['FieldB'],'value':text,'input_mode':'background'})
                times.append(round((time.monotonic()-start)*1000,1));s=wait_state(lambda s:s['b']==text)
                assert s['b']==text and s['a']=='A_SEED',s
            no_interference(before)
            return {'runs':20,'min_ms':min(times),'max_ms':max(times),'mean_ms':round(sum(times)/len(times),1),'final_b':s['b']}
        record('background_targeted_unicode_input_20_runs',set_value_test)
        def implicit_type_test():
            command('reset');command('focus-b');command('cover');_,idx=observation();before=context()
            try:
                payload,_=client.tool('type_text',{'window':window,'text':'__APPEND__','input_mode':'background'})
            except RuntimeError as exc:
                state=read_state();assert state['a']=='A_SEED' and state['b']=='B_SEED',state
                return {'safely_refused':str(exc)}
            time.sleep(.15);state=read_state();no_interference(before)
            assert state['a']=='A_SEED' and '__APPEND__' in state['b'],{'state':state,'backend':payload}
            return {'a':state['a'],'b':state['b']}
        record('ambiguous_type_must_not_write_first_field',implicit_type_test)
        def explicit_type_test():
            command('reset');command('focus-b');command('cover');_,idx=observation();before=context()
            payload,_=client.tool('type_text',{'window':window,'element_index':idx['FieldB'],'text':'中文','input_mode':'background'})
            state=wait_state(lambda s:'中文' in s['a'] or '中文' in s['b'])
            assert state['a']=='A_SEED' and '中文' in state['b'],state
            no_interference(before)
            return {'a':state['a'],'b':state['b'],'backend':payload.get('backend')}
        record('explicit_type_targets_selected_field',explicit_type_test)
        def stale_test():
            command('reset');_,idx=observation();old_b=idx['FieldB'];command('insert');before=context()
            try:client.tool('set_value',{'window':window,'element_index':old_b,'value':'INTENDED_FOR_B','input_mode':'background'})
            except RuntimeError as exc:
                state=read_state();assert state['a']=='A_SEED' and state['b']=='B_SEED' and state['injected']=='DO_NOT_TOUCH',state
                return {'safely_refused':str(exc)}
            time.sleep(.15);state=read_state();no_interference(before)
            assert state['a']=='A_SEED' and state['b']=='INTENDED_FOR_B' and state['injected']=='DO_NOT_TOUCH',state
            return {'correct_target_preserved':True,'a':state['a'],'b':state['b'],'injected':state['injected']}
        record('tree_insertion_must_not_retarget_old_index',stale_test)
        def right_click_test():
            command('reset');_,idx=observation()
            try:client.tool('click_element',{'window':window,'element_index':idx['CommitB'],'mouse_button':'right','input_mode':'background'})
            except RuntimeError as exc:return {'safely_refused':str(exc)}
            time.sleep(.15);state=read_state()
            assert state['bClicks']==0,'right click silently executed a primary button action'
            return {'bClicks':state['bClicks']}
        record('right_click_must_not_become_primary_action',right_click_test)
        def minimized_test():
            command('minimize')
            try:snap(True)
            except RuntimeError as exc:
                assert 'minimized' in str(exc).lower(),str(exc)
                return {'explicit_error':str(exc)}
            finally:command('restore');command('cover')
            raise AssertionError('expected explicit unsupported minimized capture')
        record('minimized_capture_returns_explicit_error',minimized_test)
        def budget_test():
            # Fixture intentionally blocks only its own UI for 6.5 seconds.
            command('hang');start=time.monotonic();error=None
            try:client.tool('get_window_state',{'window':window,'include_screenshot':False,'include_text':True,'input_mode':'background'},timeout=12,meta={'x-oai-cua-request-budget-ms':500})
            except RuntimeError as exc:error=str(exc)
            elapsed=time.monotonic()-start
            assert error and elapsed<1.5,{'elapsed_s':round(elapsed,3),'error':error,'budget_ms':500}
            return {'elapsed_s':elapsed,'error':error}
        record('hung_app_respects_500ms_request_budget',budget_test)
        if args.supervise:
            def cancel_test():
                ticks=read_state()['ticks'];wait_state(lambda s:s['ticks']>ticks,timeout=8)
                command('hang');ident=client.seq+1
                def notify():
                    client.p.stdin.write(json.dumps({'jsonrpc':'2.0','method':'notifications/cancelled','params':{'requestId':ident,'reason':'lab cancellation'}})+'\n');client.p.stdin.flush()
                timer=threading.Timer(.15,notify);timer.start();start=time.monotonic();error=None
                try:client.tool('get_window_state',{'window':window,'include_screenshot':False,'include_text':True},timeout=3)
                except RuntimeError as exc:error=str(exc)
                finally:timer.join(timeout=1)
                elapsed=time.monotonic()-start
                assert error and 'cancelled' in error and elapsed<1.5,{'error':error,'elapsed_s':elapsed}
                return {'elapsed_ms':round(elapsed*1000,1),'error':error}
            record('out_of_band_mcp_cancel_while_app_hung',cancel_test)
            def restart_test():
                ticks=read_state()['ticks'];wait_state(lambda s:s['ticks']>ticks,timeout=8)
                error=None
                try:client.tool('click_element',{'window':window,'element_index':6,'input_mode':'background'})
                except RuntimeError as exc:error=str(exc)
                assert error and 'refresh_required' in error,error
                _,idx=observation();before=read_state()['bClicks']
                client.tool('click_element',{'window':window,'element_index':idx['CommitB'],'input_mode':'background'})
                state=wait_state(lambda s:s['bClicks']==before+1)
                return {'stale_input_refused':error,'click_after_refresh':state['bClicks']}
            record('worker_restart_requires_refresh_and_recovers',restart_test)
    except Exception as exc:
        results.append({'test':'harness','status':'FAIL','detail':str(exc),'traceback':traceback.format_exc()});print(traceback.format_exc(),flush=True)
    finally:
        if client:client.close()
        if fixture and fixture.poll() is None:
            try:command('close');fixture.wait(timeout=8)
            except Exception:fixture.kill();fixture.wait()
        (out/'results.json').write_text(json.dumps(results,ensure_ascii=False,indent=2),encoding='utf-8')
        print('RESULTS '+str(out/'results.json'),flush=True)

if __name__=='__main__': main()
