"""Experimental stdio MCP process supervisor for the native helper.

Kills only its own helper on timeout/cancellation; never kills target apps.
Input outcomes after termination are UNKNOWN and must never be auto-retried.
A restarted helper requires a fresh window observation before input.
"""
import argparse,json,os,pathlib,queue,subprocess,sys,threading,time

ROOT=pathlib.Path(__file__).resolve().parent.parent
os.environ.setdefault('OCU_REPLICA_HOME',str(ROOT/'state'))
os.environ['OCU_INPUT_MODE']='background'
os.environ['OCU_ALLOW_POSTMESSAGE']='0'

ap=argparse.ArgumentParser();ap.add_argument('--exe',default=str(ROOT/'bin'/'ocu-native.exe'));args=ap.parse_args()
requests=queue.Queue();cancelled=set();cancel_lock=threading.Lock()
def emit(obj):
    print(json.dumps(obj,ensure_ascii=False),flush=True)
def reader():
    for line in sys.stdin:
        try:
            obj=json.loads(line)
            if obj.get('method')=='notifications/cancelled':
                with cancel_lock:cancelled.add(obj.get('params',{}).get('requestId'))
            else:requests.put(obj)
        except ValueError:emit({'jsonrpc':'2.0','id':None,'error':{'code':-32700,'message':'invalid JSON'}})
    requests.put(None)
threading.Thread(target=reader,daemon=True).start()

class Interrupted(Exception):pass
class Worker:
    def __init__(self):
        self.responses=queue.Queue();self.fresh=set()
        self.proc=subprocess.Popen([args.exe,'--parent-pid',str(os.getpid())],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=sys.stderr,text=True,encoding='utf-8',creationflags=subprocess.CREATE_NO_WINDOW)
        threading.Thread(target=self.pump,daemon=True).start()
    def pump(self):
        for line in self.proc.stdout:
            try:self.responses.put(json.loads(line))
            except ValueError:pass
        self.responses.put({'workerExited':True})
    def send(self,req):
        self.proc.stdin.write(json.dumps(req,ensure_ascii=False)+'\n');self.proc.stdin.flush()
    def wait(self,ident,deadline,cancel_id):
        while True:
            with cancel_lock:is_cancelled=cancel_id in cancelled
            if is_cancelled:raise Interrupted('cancelled')
            remaining=deadline-time.monotonic()
            if remaining<=0:raise Interrupted('deadline_exceeded')
            try:response=self.responses.get(timeout=min(.025,remaining))
            except queue.Empty:continue
            if response.get('workerExited'):raise Interrupted('worker_exited')
            if response.get('id')==ident:return response
    def kill(self):
        if self.proc.poll() is None:self.proc.kill()
        try:self.proc.wait(timeout=.5)
        except subprocess.TimeoutExpired:pass
        self.fresh.clear()

READ_ONLY={'list_apps','list_windows','get_window','get_window_state','list_displays','diagnostic_state','browser_list_tabs','browser_get_state','browser_screenshot','end_turn','close','scope_monitor'}
worker=None;saved_init=None
try:
    while True:
        request=requests.get()
        if request is None:break
        ident=request.get('id');method=request.get('method');params=request.get('params') or {}
        if ident is None:
            if worker:worker.send(request)
            continue
        meta=params.get('_meta',params.get('meta',{})) or {}
        budget=meta.get('x-oai-cua-request-budget-ms',12000)
        try:budget=max(1,min(float(budget),120000))
        except (TypeError,ValueError):budget=12000
        deadline=time.monotonic()+budget/1000
        try:
            if worker is None:
                worker=Worker()
                if saved_init and method!='initialize':
                    internal=dict(saved_init);internal['id']='__supervisor_reinitialize__'
                    worker.send(internal);worker.wait(internal['id'],deadline,ident)
            tool=params.get('name');arguments=params.get('arguments') or {};window=arguments.get('window') or {}
            if method=='tools/call' and tool not in READ_ONLY and window.get('id') not in worker.fresh:
                emit({'jsonrpc':'2.0','id':ident,'result':{'isError':True,'content':[{'type':'text','text':'refresh_required: call get_window_state after starting or restarting the worker before input'}]}})
                continue
            worker.send(request);response=worker.wait(ident,deadline,ident)
            if method=='initialize' and 'result' in response:saved_init=request
            if method=='tools/call' and tool=='get_window_state' and not response.get('result',{}).get('isError',True):worker.fresh.add(window.get('id'))
            emit(response)
        except (Interrupted,OSError) as exc:
            if worker:worker.kill()
            worker=None
            error={'status':'outcome_unknown','reason':str(exc),'worker_terminated':True,'refresh_required':True,'auto_retry':False}
            if method=='tools/call':emit({'jsonrpc':'2.0','id':ident,'result':{'isError':True,'content':[{'type':'text','text':json.dumps(error)}]}})
            else:emit({'jsonrpc':'2.0','id':ident,'error':{'code':-32001,'message':json.dumps(error)}})
        finally:
            with cancel_lock:cancelled.discard(ident)
finally:
    if worker:worker.kill()
