"""Fake native helper/private descriptor tests; never invoke real Codescribe keys."""
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

SCRIPTS=Path(__file__).resolve().parents[1]
def load(name,path):
    spec=importlib.util.spec_from_file_location(name,path)
    module=importlib.util.module_from_spec(spec);sys.modules[name]=module;spec.loader.exec_module(module)
    return module
BASE=load('native_key_test_base',SCRIPTS/'tests/test_bus_mcp_agent.py')
KEYS=load('native_key_test_driver',SCRIPTS/'bus-mcp-key-store.py')
RUNTIME=load('native_key_test_runtime',SCRIPTS/'bus-mcp-runtime.py')


class NativeFixture(BASE.Fixture):
    def setUp(self):
        super().setUp()
        self.helper=self.root/'fake-codescribe'
        self.helper.write_text('#!'+sys.executable+'\n'+f'''
import json,os,pathlib,socket,sys
root=pathlib.Path({str(self.root)!r})
fd=int(sys.argv[sys.argv.index('--fd')+1])
with socket.socket(fileno=fd) as channel:
    raw=b''
    while True:
        part=channel.recv(2049-len(raw))
        if not part:break
        raw+=part
    request=json.loads(raw)
    (root/'metadata.json').write_text(json.dumps({{'argv':sys.argv[1:],'env_keys':sorted(os.environ)}}))
    registry=root/'fake-refs.json'
    refs=json.loads(registry.read_text()) if registry.exists() else []
    op=request['op'];ref=request.get('reference')
    if op=='put':
        if ref not in refs:refs.append(ref)
        response={{'ok':True}}
    elif op=='get':response={{'ok':True,'secret':{BASE.SECRET!r}}} if ref in refs else {{'ok':False,'error':'not_found'}}
    elif op=='delete':refs=[r for r in refs if r!=ref];response={{'ok':True}}
    else:response={{'ok':True,'references':refs}}
    registry.write_text(json.dumps(refs))
    channel.sendall(json.dumps(response).encode())
''')
        self.helper.chmod(0o700)
        self.digest=hashlib.sha256(self.helper.read_bytes()).hexdigest()
        self.namespace=self.events.namespace
        self.ref=self.namespace+'a'*32
        self.store=KEYS.CodescribeSecretStore(self.helper,self.namespace,self.digest)
        self.addCleanup(self.store.close)

    def replace_helper(self,code,timeout=10):
        self.helper.write_text('#!'+sys.executable+'\n'+code);self.helper.chmod(0o700)
        self.digest=hashlib.sha256(self.helper.read_bytes()).hexdigest()
        store=KEYS.CodescribeSecretStore(self.helper,self.namespace,self.digest,timeout=timeout)
        self.addCleanup(store.close)
        return store


class NativeKeyTests(NativeFixture):
    def test_private_fd_roundtrip_and_no_credential_environment_or_argv(self):
        with patch.dict(os.environ,{'CONTROL_PLANE_API_KEY':'synthetic-control-plane','OPENAI_API_KEY':'synthetic-model','LLM_XAI_API_KEY':'synthetic-voice'}):
            self.store.put(self.ref,BASE.SECRET)
            self.assertEqual(self.store.get(self.ref),BASE.SECRET)
            self.assertEqual(self.store.references(self.namespace),[self.ref])
        metadata=json.loads((self.root/'metadata.json').read_text())
        self.assertFalse({'CONTROL_PLANE_API_KEY','OPENAI_API_KEY','LLM_XAI_API_KEY'} & set(metadata['env_keys']))
        self.assertEqual(metadata['argv'][:2],['mcp-event-key','--fd'])
        self.assertNotIn(BASE.SECRET,json.dumps(metadata))
        self.assertNotIn(BASE.SECRET,(self.root/'fake-refs.json').read_text())
        self.store.delete(self.ref);self.store.delete(self.ref)
        self.assertEqual(self.store.references(self.namespace),[])

    def test_foreign_namespaces_and_refs_refused_before_child_spawn(self):
        with patch.object(KEYS.subprocess,'Popen',side_effect=AssertionError('spawn forbidden')):
            for ref in ['codescribe_keychain_bundle_v1',self.ref+'extra','MCP_EVENTS_'+'b'*64+'_'+'a'*32]:
                with self.assertRaises(KEYS.NativeKeyError):self.store.get(ref)
            with self.assertRaises(KEYS.NativeKeyError):self.store.references('foreign')
        self.assertFalse((self.root/'metadata.json').exists())

    def test_changed_executable_refused_before_spawn(self):
        self.helper.write_text(self.helper.read_text()+'\n# changed\n')
        with patch.object(KEYS.subprocess,'Popen',side_effect=AssertionError('spawn forbidden')):
            with self.assertRaises(KEYS.NativeKeyError):self.store.references(self.namespace)

    def test_path_replacement_after_hash_cannot_receive_the_private_request(self):
        replacement=self.root/'unreviewed-helper'
        marker=self.root/'unreviewed-received'
        replacement.write_text('#!'+sys.executable+'\nimport pathlib\npathlib.Path('+repr(str(marker))+').write_text("received")\n')
        replacement.chmod(0o700)
        original=KEYS.subprocess.Popen
        def swap(args,**kwargs):
            replacement.replace(self.helper)
            return original(args,**kwargs)
        with patch.object(KEYS.subprocess,'Popen',side_effect=swap):
            self.store.put(self.ref,BASE.SECRET)
        self.assertFalse(marker.exists())
        self.assertEqual(json.loads((self.root/'fake-refs.json').read_text()),[self.ref])

    def test_inplace_overwrite_after_hash_cannot_change_the_checked_image(self):
        marker=self.root/'unreviewed-inplace'
        original=KEYS.subprocess.Popen
        def rewrite(args,**kwargs):
            self.helper.write_text('#!'+sys.executable+'\nimport pathlib\npathlib.Path('+repr(str(marker))+').write_text("received")\n')
            return original(args,**kwargs)
        with patch.object(KEYS.subprocess,'Popen',side_effect=rewrite):
            self.store.put(self.ref,BASE.SECRET)
        self.assertFalse(marker.exists())
        self.assertEqual(json.loads((self.root/'fake-refs.json').read_text()),[self.ref])

    def test_snapshot_is_private_read_only_and_removed_with_the_binding(self):
        import fcntl
        self.assertEqual(fcntl.fcntl(self.store.image.fileno(),fcntl.F_GETFL)&os.O_ACCMODE,os.O_RDONLY)
        self.assertEqual(self.store.image_path.stat().st_mode&0o777,0o500)
        self.assertEqual(self.store.image_path.parent.stat().st_mode&0o777,0o700)
        self.assertNotEqual(self.store.image_path,self.helper)
        self.store.close();self.store.close()
        self.assertTrue(self.store.image.closed)
        self.assertFalse(self.store.image_path.exists())
        with self.assertRaises(KEYS.NativeKeyError):self.store.references(self.namespace)

    def test_malformed_response_is_categorized_without_hidden_data(self):
        store=self.replace_helper("import os,socket,sys\nfd=int(sys.argv[-1])\nwith socket.socket(fileno=fd) as s:\n s.recv(4096)\n s.sendall(b'private-value-not-json')\n")
        with self.assertRaises(KEYS.NativeKeyError) as error:store.get(self.ref)
        self.assertNotIn('private-value',str(error.exception))

    def test_timeout_reaps_only_own_child_and_never_logs_payload(self):
        store=self.replace_helper('import time\ntime.sleep(2)\n',timeout=.05)
        with self.assertRaises(KEYS.NativeKeyError) as error:store.get(self.ref)
        self.assertNotIn(BASE.SECRET,str(error.exception))

    def test_oversized_response_is_refused(self):
        store=self.replace_helper("import socket,sys\nwith socket.socket(fileno=int(sys.argv[-1])) as s:\n s.recv(4096)\n s.sendall(b'x'*32769)\n")
        with self.assertRaises(KEYS.NativeKeyError):store.get(self.ref)

    def test_bootstrap_wires_native_store_and_events_with_fake_callback(self):
        scope=self.root/'scope.json';scope.write_text(json.dumps(self.config))
        requests=[{'jsonrpc':'2.0','id':1,'method':'server/discover'},
                  {'jsonrpc':'2.0','id':2,'method':'events/subscribe','params':{'_meta':BASE.META,**self.subscription()}}]
        stdin=type('Input',(),{'buffer':io.BytesIO((''.join(json.dumps(r)+'\n' for r in requests)).encode())})()
        output=io.StringIO();captured=[]
        original=RUNTIME.configure
        case=self
        class Callback:
            def resolve(self,host):return ['1.1.1.1']
            def __call__(self,*args):return case.callback(*args)
        def configure(server,events,store,**kwargs):
            captured.append(server)
            kwargs.update(transport=Callback(),clock=lambda:self.now[0],
                timer_factory=lambda delay,callback:BASE.FakeTimer(self.alarms,delay,callback))
            original(server,events,store,**kwargs)
            return type('Lifecycle',(),{'start':lambda inner:None,'stop':lambda inner:server.events.close()})()
        with patch.object(sys,'stdin',stdin),patch.object(sys,'stdout',output),patch.object(RUNTIME,'configure',side_effect=configure):
            RUNTIME.main(['--scope',str(scope),'--codescribe',str(self.helper),'--native-sha256',self.digest,
                          '--callback-host','callback.example.test'])
        result=[json.loads(line) for line in output.getvalue().splitlines()]
        self.assertIn('events',result[0]['result']['capabilities'])
        self.assertIn('id',result[1]['result'])
        self.assertEqual(len(self.posts),1)
        self.assertTrue(captured[0].events.closed)
        self.assertNotIn(BASE.SECRET,output.getvalue())
        data=(captured[0].calls.parent/'events.sqlite').read_bytes()
        self.assertNotIn(BASE.SECRET.encode(),data)
        self.assertFalse((self.root/'acknowledgments').exists())


if __name__=='__main__':unittest.main()
