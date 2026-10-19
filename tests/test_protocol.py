"""No-GUI checks against the actual retained MCP server and native frontend."""
import json,os,pathlib,subprocess,sys,unittest
ROOT=pathlib.Path(__file__).resolve().parent.parent
EXPECTED={'list_apps','list_windows','get_window','get_window_state','click_element','type_text','set_value','diagnostic_state','end_turn','close','list_displays'}

class ProtocolTests(unittest.TestCase):
    def test_mcp_discovery_and_removed_methods(self):
        requests=[
            {'jsonrpc':'2.0','id':1,'method':'initialize','params':{'protocolVersion':'2025-03-26'}},
            {'jsonrpc':'2.0','id':2,'method':'tools/list','params':{}},
            {'jsonrpc':'2.0','id':3,'method':'tools/call','params':{'name':'browser_list_tabs','arguments':{}}},
            {'jsonrpc':'2.0','id':4,'method':'tools/call','params':{'name':'list_windows','arguments':{'input_mode':'foreground'}}},
        ]
        result=subprocess.run([sys.executable,str(ROOT/'runtime/supervisor.py')],input=''.join(json.dumps(r)+'\n' for r in requests),capture_output=True,text=True,encoding='utf-8',timeout=12,creationflags=subprocess.CREATE_NO_WINDOW)
        self.assertEqual(result.returncode,0,result.stderr)
        responses={v['id']:v for v in map(json.loads,result.stdout.splitlines())}
        tools=responses[2]['result']['tools'];self.assertEqual({t['name'] for t in tools},EXPECTED)
        typing=next(t for t in tools if t['name']=='type_text')
        self.assertIn('element_index',typing['inputSchema']['required'])
        self.assertTrue(responses[3]['result']['isError'])
        self.assertTrue(responses[4]['result']['isError'])

    def test_native_protocol_also_rejects_removed_methods(self):
        env=os.environ.copy();env['OCU_REPLICA_HOME']=str(ROOT/'tests'/'native-protocol-state')
        requests=[{'id':1,'method':'browser_list_tabs','params':{}},{'id':2,'method':'press_key','params':{}},{'id':3,'method':'close','params':{}}]
        result=subprocess.run([str(ROOT/'bin/ocu-native.exe'),'--stdio-native'],input=''.join(json.dumps(r)+'\n' for r in requests),capture_output=True,text=True,encoding='utf-8',timeout=8,env=env,creationflags=subprocess.CREATE_NO_WINDOW)
        self.assertEqual(result.returncode,0,result.stderr)
        responses=list(map(json.loads,result.stdout.splitlines()))
        for response in responses[:2]:
            self.assertFalse(response['ok']);self.assertIn('not supported',response['error'])
        self.assertTrue(responses[2]['ok'])

if __name__=='__main__':unittest.main(verbosity=2)
