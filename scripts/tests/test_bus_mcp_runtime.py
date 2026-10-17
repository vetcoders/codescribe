"""Own-mailbox Events integration: synthetic key store, DNS/TLS/socket and callback."""
import base64
import contextlib
import concurrent.futures
import io
import json
import socket
import sqlite3
import time
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch
import importlib.util
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('runtime_test_fixture', SCRIPTS / 'tests/test_bus_mcp_agent.py')
BASE = importlib.util.module_from_spec(spec)
spec.loader.exec_module(BASE)
RUNTIME = BASE.load('tested_bus_runtime', 'bus-mcp-runtime.py')
EVENTS = BASE.EVENTS
SECRET, PRINCIPAL, TIME = BASE.SECRET, BASE.PRINCIPAL, BASE.TIME
NEW_SECRET = 'whsec_' + base64.b64encode(b'new-synthetic-signing-key-000000').decode()
THIRD_SECRET = 'whsec_' + base64.b64encode(b'third-synthetic-signing-key-0000').decode()


class SecretStoreTests(BASE.Fixture):
    def test_active_signing_material_never_enters_metadata_and_restart_keeps_refs(self):
        self.subscribe()
        ref = self.events.db.execute('SELECT current_key_ref FROM subscriptions').fetchone()[0]
        self.assertEqual(self.secret_store.values[ref], SECRET)
        data = (self.root / 'synthetic-events.sqlite').read_bytes()
        self.assertNotIn(SECRET.encode(), data)
        self.assertNotIn(EVENTS.signing_key(SECRET), data)
        self.events.close()
        resumed = self.new_events()
        self.assertEqual(resumed.key(ref), SECRET)
        resumed.deliver(PRINCIPAL, self.ids[0], TIME)
        self.assert_signature(self.posts[-1][2], self.posts[-1][3])

    def test_single_metadata_owner_refuses_duplicate_connection(self):
        self.subscribe()
        with self.assertRaises(EVENTS.SecretStoreError):
            self.new_events()
        self.assertEqual(len(self.secret_store.values), 1)

    def test_metadata_lock_never_follows_symlink_to_unrelated_file(self):
        unrelated=self.root/'unrelated';unrelated.write_text('keep')
        (self.root/'blocked.lock').symlink_to(unrelated)
        with self.assertRaises(EVENTS.SecretStoreError):
            EVENTS.Events(self.root/'blocked.sqlite',PRINCIPAL,BASE.SESSION,secret_store=self.secret_store)
        self.assertEqual(unrelated.read_text(),'keep')

    def test_three_rotations_remove_superseded_old_key_and_bound_overlap(self):
        self.subscribe()
        p = self.subscription()
        p['delivery']['secret'] = NEW_SECRET
        self.events.subscribe(PRINCIPAL, p)
        p['delivery']['secret'] = THIRD_SECRET
        self.events.subscribe(PRINCIPAL, p)
        self.assertEqual(set(self.secret_store.values.values()), {NEW_SECRET, THIRD_SECRET})
        self.now[0] += 61
        self.events.maintenance()
        self.assertEqual(list(self.secret_store.values.values()), [THIRD_SECRET])

    def test_failed_key_delete_deactivates_metadata_and_retries_cleanup_before_delivery(self):
        self.subscribe()
        p = self.subscription(); p.pop('cursor'); p['delivery'].pop('secret')
        before = len(self.posts)
        old_timer = self.events.timer
        with patch.object(self.secret_store, 'delete', side_effect=OSError('private backend detail')):
            with self.assertRaises(EVENTS.SecretStoreError) as error:
                self.events.unsubscribe(PRINCIPAL, p)
            self.assertNotIn('private', str(error.exception))
            self.assertEqual(self.events.db.execute('SELECT COUNT(*) FROM subscriptions').fetchone()[0], 0)
            self.assertEqual(self.events.db.execute('SELECT COUNT(*) FROM cleanup').fetchone()[0], 1)
            with self.assertRaises(EVENTS.SecretStoreError):
                self.events.deliver(PRINCIPAL, self.ids[0], TIME)
        self.assertIsNot(self.events.timer, old_timer)
        self.assertEqual(self.events.timer.delay, 1)
        self.events.timer.fire()
        self.assertFalse(self.secret_store.values)
        self.assertEqual(self.events.db.execute('SELECT COUNT(*) FROM cleanup').fetchone()[0], 0)
        self.assertEqual(len(self.posts), before)

    def test_partial_key_write_is_compensated_without_subscription(self):
        original = self.secret_store.put
        def partial(ref, secret):
            original(ref, secret)
            raise OSError('synthetic backend failure')
        with patch.object(self.secret_store, 'put', side_effect=partial):
            with self.assertRaises(EVENTS.SecretStoreError): self.subscribe()
        self.assertEqual(self.events.db.execute('SELECT COUNT(*) FROM subscriptions').fetchone()[0], 0)
        self.events.maintenance()
        self.assertFalse(self.secret_store.values)

    def test_failed_metadata_write_keeps_old_subscription_and_compensates_new_key(self):
        self.subscribe()
        self.events.db.execute("CREATE TRIGGER refuse_insert BEFORE INSERT ON subscriptions BEGIN SELECT RAISE(ABORT,'synthetic'); END")
        p=self.subscription();p['delivery']['secret']=NEW_SECRET
        with self.assertRaises(EVENTS.SecretStoreError): self.events.subscribe(PRINCIPAL,p)
        self.assertEqual(list(self.secret_store.values.values()), [SECRET])
        ref=self.events.db.execute('SELECT current_key_ref FROM subscriptions').fetchone()[0]
        self.assertEqual(self.events.key(ref), SECRET)

    def test_restart_erases_only_own_orphans_and_expired_keys(self):
        self.subscribe(ttlMs=1000)
        namespace=self.events.namespace
        self.secret_store.put(namespace+'orphan',NEW_SECRET)
        self.secret_store.put('UNRELATED_FOREIGN_KEY',THIRD_SECRET)
        self.events.close();self.now[0]+=2
        self.new_events()
        self.assertEqual(self.secret_store.values, {'UNRELATED_FOREIGN_KEY':THIRD_SECRET})

    def test_missing_key_fails_closed_without_callback_or_plaintext_recovery(self):
        self.subscribe();self.secret_store.values.clear();before=len(self.posts)
        with self.assertRaises(EVENTS.SecretStoreError):self.events.deliver(PRINCIPAL,self.ids[0],TIME)
        self.assertEqual(len(self.posts),before)
        self.assertNotIn(SECRET.encode(),(self.root/'synthetic-events.sqlite').read_bytes())

    def test_three_key_read_outages_spend_zero_http_attempts_then_recover_after_restart(self):
        self.subscribe();before=len(self.posts)
        with patch.object(self.secret_store,'get',side_effect=OSError('private key-store detail')):
            for _ in range(3):
                with self.assertRaises(EVENTS.SecretStoreError):
                    self.events.deliver(PRINCIPAL,self.ids[0],TIME)
                self.now[0]+=10
            self.assertEqual(self.events.db.execute('SELECT COUNT(*) FROM deliveries').fetchone()[0],0)
            self.assertEqual(len(self.posts),before)
        self.events.close();resumed=self.new_events()
        self.assertTrue(resumed.ready(PRINCIPAL,self.ids[0]))
        result=resumed.deliver(PRINCIPAL,self.ids[0],TIME)
        self.assertEqual((result[0]['attempt'],result[0]['state']),(1,'accepted'))
        self.assertEqual(len(self.posts),before+1)

    def test_key_outage_preserves_existing_http_retry_budget_and_body(self):
        self.subscribe();self.statuses=[503,202]
        self.events.deliver(PRINCIPAL,self.ids[0],TIME);body=self.posts[-1][2]
        before=len(self.posts)
        with patch.object(self.secret_store,'get',side_effect=OSError('private detail')):
            for _ in range(3):
                self.now[0]+=10
                with self.assertRaises(EVENTS.SecretStoreError):self.events.deliver(PRINCIPAL,self.ids[0],TIME)
            self.assertEqual(self.events.db.execute('SELECT attempts FROM deliveries').fetchone()[0],1)
            self.assertEqual(len(self.posts),before)
        self.events.close();resumed=self.new_events()
        self.assertTrue(resumed.ready(PRINCIPAL,self.ids[0]))
        result=resumed.deliver(PRINCIPAL,self.ids[0],TIME)
        self.assertEqual((result[0]['attempt'],result[0]['state']),(2,'accepted'))
        self.assertEqual(self.posts[-1][2],body)

    def test_stable_serialized_body_survives_changed_retry_timestamp_and_restart(self):
        self.subscribe();self.statuses=[503,202]
        self.events.deliver(PRINCIPAL,self.ids[0],TIME)
        body=self.posts[-1][2]
        self.now[0]+=3;self.events.close();resumed=self.new_events()
        resumed.deliver(PRINCIPAL,self.ids[0],'2026-10-14T12:15:00Z')
        self.assertEqual(self.posts[-1][2],body)
        self.assert_signature(body,self.posts[-1][3])

    def test_backend_required_and_plaintext_prototype_is_never_admitted(self):
        with self.assertRaises(EVENTS.SecretStoreError):
            EVENTS.Events(self.root/'absent.sqlite',PRINCIPAL,BASE.SESSION)
        self.assertFalse((self.root/'absent.sqlite').exists())
        self.events.close()
        path=self.root/'old.sqlite'
        with contextlib.closing(sqlite3.connect(path)) as db:
            db.execute('CREATE TABLE subscriptions (id TEXT,secret TEXT,old_secret TEXT)')
        with self.assertRaises(EVENTS.SecretStoreError):
            EVENTS.Events(path,PRINCIPAL,BASE.SESSION,secret_store=self.secret_store)
        self.assertFalse(self.secret_store.values)

    def test_second_callback_destination_requires_explicit_unsubscribe(self):
        self.subscribe();p=self.subscription();p['delivery']['url']=BASE.URL+'-other'
        before=len(self.posts)
        with self.assertRaises(ValueError):self.events.subscribe(PRINCIPAL,p)
        self.assertEqual(len(self.posts),before)
        self.assertEqual(len(self.secret_store.values),1)


class HTTPSCallbackTests(unittest.TestCase):
    def factory(self,status=202,data=b'{}'):
        self.calls=[]
        def build(host,address,deadline,**kwargs):
            item=SimpleNamespace(host=host,address=address,deadline=deadline,closed=False)
            def request(*args,**kw):item.request=(args,kw)
            def close():item.closed=True
            item.request_fn=request;item.close=close
            connection=SimpleNamespace(request=request,close=close,
                getresponse=lambda:SimpleNamespace(status=status,read=lambda n:data[:n],close=lambda:None))
            self.calls.append(item)
            return connection
        return build

    def test_dns_pin_hostname_tls_and_query_without_real_network(self):
        factory=self.factory()
        transport=RUNTIME.HTTPSCallback(lookup=lambda *a,**kw:[(socket.AF_INET,socket.SOCK_STREAM,0,'',('1.1.1.1',443))],connection_factory=factory)
        with patch.object(socket,'socket',side_effect=AssertionError('real network forbidden')):
            addresses=transport.resolve('callback.example.test')
            self.assertEqual(transport(BASE.URL+'?nonce=synthetic',addresses,b'{}',{}),(202,{}))
        call=self.calls[0]
        self.assertEqual((call.host,call.address),('callback.example.test','1.1.1.1'))
        self.assertEqual(call.request[0][1],'/agent?nonce=synthetic')
        self.assertTrue(call.closed)

    def test_private_dns_and_redirects_are_not_followed(self):
        factory=self.factory(status=302)
        transport=RUNTIME.HTTPSCallback(lookup=lambda *a,**kw:[(0,0,0,'',('127.0.0.1',443))],connection_factory=factory)
        with self.assertRaises(ValueError):transport.resolve('callback.example.test')
        self.assertFalse(self.calls)
        self.assertEqual(transport(BASE.URL,('1.1.1.1',),b'{}',{}),(302,{}))
        self.assertEqual(len(self.calls),1)
        self.assertTrue(self.calls[0].closed)

    def test_dns_pending_worker_is_bounded_and_response_size_is_bounded(self):
        transport=RUNTIME.HTTPSCallback(connection_factory=self.factory(data=b'x'*65537))
        transport.pending=concurrent.futures.Future()
        with self.assertRaises(TimeoutError):transport.resolve('callback.example.test')
        with self.assertRaises(ValueError):transport(BASE.URL,('1.1.1.1',),b'{}',{})
        self.assertTrue(self.calls[0].closed)

    def test_tls_connects_only_pinned_ip_with_original_sni(self):
        raw=Mock();secure=Mock()
        context=Mock();context.wrap_socket.return_value=secure
        connection=RUNTIME.PinnedHTTPSConnection('callback.example.test','2606:4700:4700::1111',10,
            clock=lambda:1,context=context,socket_factory=lambda *a:raw)
        connection.connect()
        raw.connect.assert_called_once_with(('2606:4700:4700::1111',443))
        context.wrap_socket.assert_called_once_with(raw,server_hostname='callback.example.test')
        connection.close();secure.close.assert_called_once()

    def test_real_http_parser_keeps_split_body_alive_for_connection_close_and_keepalive(self):
        body=b'{"challenge":"synthetic"}'
        for policy in ['close','keep-alive']:
            with self.subTest(connection=policy):
                class FakeTLS:
                    def __init__(self):
                        self.chunks=[('HTTP/1.1 200 OK\r\nContent-Length: '+str(len(body))+
                            '\r\nConnection: '+policy+'\r\n\r\n').encode(),body]
                        self.closed=False;self.close_count=0
                    def settimeout(self,value):pass
                    def connect(self,address):pass
                    def sendall(self,data):pass
                    def recv_into(self,buffer):
                        if self.closed:raise OSError('socket closed before body')
                        if not self.chunks:return 0
                        chunk=self.chunks.pop(0)
                        count=min(len(buffer),len(chunk));buffer[:count]=chunk[:count]
                        if count<len(chunk):self.chunks.insert(0,chunk[count:])
                        return count
                    def close(self):self.closed=True;self.close_count+=1
                sock=FakeTLS()
                def factory(host,address,deadline,**kwargs):
                    return RUNTIME.PinnedHTTPSConnection(host,address,deadline,
                        context=SimpleNamespace(wrap_socket=lambda raw,**kw:raw),
                        socket_factory=lambda *a:sock,**kwargs)
                transport=RUNTIME.HTTPSCallback(connection_factory=factory)
                self.assertEqual(transport(BASE.URL,('1.1.1.1',),b'{}',{}),
                    (200,{'challenge':'synthetic'}))
                self.assertTrue(sock.closed)
                self.assertEqual(sock.close_count,1)
                self.assertFalse(sock.chunks)

    def test_deadline_expires_before_read_send_or_connect(self):
        sock=Mock()
        reader=RUNTIME.DeadlineReader(sock,1,lambda:2)
        with self.assertRaises(TimeoutError):reader.readinto(bytearray(8))
        with self.assertRaises(TimeoutError):RUNTIME.DeadlineSocket(sock,1,lambda:2).sendall(b'x')
        sock.recv_into.assert_not_called();sock.sendall.assert_not_called()
        factory=Mock()
        connection=RUNTIME.PinnedHTTPSConnection('callback.example.test','1.1.1.1',1,clock=lambda:2,socket_factory=factory)
        with self.assertRaises(TimeoutError):connection.connect()
        factory.assert_not_called()


class RuntimeLifecycleTests(BASE.Fixture):
    def test_preflight_canonical_bus_mismatch_precedes_callback_and_scope_alignment_restores_host(self):
        self.events.close()
        # The observed root envelope had empty arguments, standard metadata,
        # and two fields beyond name/arguments/_meta: delivery and cursor.
        channel_bus = self.root / 'buses/channel-2.jsonl'
        channel_bus.parent.mkdir(); channel_bus.touch()
        self.state['bus'] = str(channel_bus)
        for row in self.state['pending']:
            row['bus'] = str(channel_bus)
        self.save()
        pump = RUNTIME.configure(self.server, EVENTS, None, callback_hosts=(), callback_preflight=True)
        self.addCleanup(pump.stop)
        request = {'jsonrpc': '2.0', 'id': 1, 'method': 'events/subscribe',
                   'params': {'_meta': {**BASE.META, 'io.modelcontextprotocol/clientInfo': {
                       'name': 'SyntheticClient', 'version': '1'}}, **self.subscription()}}
        payload = json.dumps(request) + '\n'
        output, errors = io.StringIO(), io.StringIO()
        def run():
            output.seek(0); output.truncate(); errors.seek(0); errors.truncate()
            stream = SimpleNamespace(buffer=io.BytesIO(payload.encode()))
            with patch.object(BASE.AGENT.sys, 'stdin', stream), patch.object(BASE.AGENT.sys, 'stdout', output), \
                    patch.object(BASE.AGENT.sys, 'stderr', errors):
                BASE.AGENT.serve(self.server, self.server.scope)
            return json.loads(output.getvalue()), json.loads(errors.getvalue())
        result, trace = run()
        self.assertEqual(result['error']['code'], -32602)
        self.assertEqual(result['error']['data'], {'reason': 'mailbox_bus_mismatch'})
        self.assertEqual((trace['stage'], trace['method'], trace['params_unknown_count']),
                         ('dispatch', 'events/subscribe', 2))
        self.assertTrue(trace['empty_arguments'])
        self.assertNotIn('callbackHost', trace)
        # Only the reviewed LOCAL scope changes. The canonical owner guard,
        # lease, pending IDs, cursor and ACK state are untouched.
        pump.stop()
        before = (self.root / 'leases' / (self.lease + '.json')).read_bytes()
        self.server = BASE.AGENT.AgentMailbox(BASE.DEMUX, self.native,
                         BASE.AGENT.Scope({**self.config, 'bus': str(channel_bus)}))
        pump = RUNTIME.configure(self.server, EVENTS, None, callback_hosts=(), callback_preflight=True)
        self.addCleanup(pump.stop)
        result, trace = run()
        self.assertEqual(result['error']['code'], -32015)
        self.assertEqual(trace['reason'], 'host_unpinned')
        self.assertEqual(trace['callbackHost'], 'callback.example.test')
        self.assertEqual(before, (self.root / 'leases' / (self.lease + '.json')).read_bytes())
        self.assertEqual((self.posts, self.alarms), ([], []))
        self.assertNotIn(BASE.URL, output.getvalue() + errors.getvalue())
        self.assertNotIn(SECRET, output.getvalue() + errors.getvalue())

    def test_preflight_has_no_backend_network_worker_or_implicit_activation(self):
        with patch.object(RUNTIME, 'HTTPSCallback', side_effect=AssertionError('no network transport')), \
                patch.object(EVENTS.sqlite3, 'connect', side_effect=AssertionError('no metadata')), \
                patch.object(RUNTIME.threading, 'Thread', side_effect=AssertionError('no watcher')):
            pump = RUNTIME.configure(self.server, EVENTS, None, callback_hosts=(), callback_preflight=True)
            pump.start()
            self.assertEqual(pump.state, 'preflight')
            self.assertIsNone(pump.thread)
            self.assertEqual(pump.tick(), [])
            with patch.object(self.native, 'status', return_value={'attached': True}):
                self.assertIn('content', self.server.dispatch(PRINCIPAL, 'tools/call',
                    {'name': 'codescribe_status', 'arguments': {}}))
            self.assertTrue(pump.stop())
            self.assertTrue(self.server.events.closed)
        self.assertFalse((self.server.calls.parent / 'events.sqlite').exists())

    def test_preflight_cannot_be_mixed_with_pins_or_secret_store_factory(self):
        for values in ({'callback_hosts': ['callback.example.test'], 'secret_store': None},
                       {'callback_hosts': (), 'secret_store': self.secret_store}):
            with self.assertRaises(ValueError):
                RUNTIME.configure(self.server, EVENTS, callback_preflight=True, **values)
        scope = self.root / 'scope.json'; scope.write_text(json.dumps(self.config))
        factory = Mock(side_effect=AssertionError('native backend forbidden'))
        with self.assertRaises(ValueError):
            RUNTIME.run(scope, callback_preflight=True, secret_store_factory=factory)
        factory.assert_not_called()
        for values in (['--callback-preflight', '--callback-host', 'callback.example.test'],
                       ['--callback-preflight', '--codescribe', '/unused-native'],
                       ['--callback-host', 'callback.example.test']):
            with patch.object(BASE.AGENT.sys, 'stderr', io.StringIO()):
                with self.assertRaises(SystemExit) as failure:
                    RUNTIME.main(['--scope', str(scope), *values])
            self.assertEqual(failure.exception.code, 2)

    def configure(self):
        transport=SimpleNamespace(resolve=lambda host:['1.1.1.1'])
        case=self
        class Callback:
            resolve=staticmethod(transport.resolve)
            def __call__(self,*args):return case.callback(*args)
        pump=RUNTIME.configure(self.server,EVENTS,self.secret_store,callback_hosts=['callback.example.test'],
            transport=Callback(),synthetic=True,clock=lambda:self.now[0],
            timer_factory=lambda delay,callback:BASE.FakeTimer(self.alarms,delay,callback))
        self.addCleanup(self.server.events.close)
        return pump

    def test_stdio_owns_runtime_start_and_eof_cleanup(self):
        scope_path=self.root/'scope.json';scope_path.write_text(json.dumps(self.config))
        requests=[{'jsonrpc':'2.0','id':1,'method':'server/discover'},
                  {'jsonrpc':'2.0','id':2,'method':'events/list','params':{'_meta':BASE.META}}]
        input_stream=SimpleNamespace(buffer=io.BytesIO((''.join(json.dumps(r)+'\n' for r in requests)).encode()))
        output=io.StringIO();ownership=[]
        def factory(server):
            real_server,self.server=self.server,server
            self.configure();self.server=real_server
            ownership.append(server)
            return SimpleNamespace(start=lambda:ownership.append('started'),
                stop=lambda:(server.events.close(),ownership.append('closed')))
        with patch.object(BASE.AGENT.sys,'stdin',input_stream),patch.object(BASE.AGENT.sys,'stdout',output):
            BASE.AGENT.main(['--scope',str(scope_path)],runtime_factory=factory)
        responses=[json.loads(r) for r in output.getvalue().splitlines()]
        self.assertIn('events',responses[0]['result']['capabilities'])
        self.assertEqual(responses[1]['result']['events'][0]['name'],EVENTS.EVENT_NAME)
        self.assertEqual(ownership[1:],['started','closed'])
        self.assertTrue(ownership[0].events.closed)
        self.assertFalse(self.posts)
        self.assertFalse(self.secret_store.values)

    def test_canonical_mailbox_refusal_still_revokes_live_subscription_and_keys(self):
        self.configure()
        self.server.dispatch(PRINCIPAL, 'events/subscribe', self.subscription())
        self.assertTrue(self.secret_store.values)
        before = len(self.posts)
        error = BASE.DEMUX.MailboxReadError('mailbox_bus_mismatch', 'fixed synthetic refusal')
        with patch.object(self.server, 'owned_pending', side_effect=error):
            with self.assertRaises(PermissionError):
                self.server.events.maintenance(PRINCIPAL)
        self.assertEqual(self.server.events.db.execute('SELECT COUNT(*) FROM subscriptions').fetchone()[0], 0)
        self.assertFalse(self.secret_store.values)
        self.assertEqual(len(self.posts), before)

    def test_stdio_callback_failures_have_protocol_code_and_only_categorized_reason(self):
        self.configure()
        for behavior,reason in [((200,{'challenge':'wrong-private-value'}),'challenge_failed'),
                                (OSError('private endpoint/key detail'),'timeout')]:
            with self.subTest(reason=reason):
                def callback(*args):
                    if isinstance(behavior,Exception):raise behavior
                    return behavior
                self.server.events.callback=callback
                request={'jsonrpc':'2.0','id':17,'method':'events/subscribe','params':{'_meta':BASE.META,**self.subscription()}}
                input_stream=SimpleNamespace(buffer=io.BytesIO((json.dumps(request)+'\n').encode()))
                output=io.StringIO()
                with patch.object(BASE.AGENT.sys,'stdin',input_stream),patch.object(BASE.AGENT.sys,'stdout',output):
                    BASE.AGENT.serve(self.server,self.server.scope)
                value=json.loads(output.getvalue())
                self.assertEqual(value['id'],17)
                self.assertEqual(value['error'],{'code':-32015,'message':'Callback verification refused',
                    'data':{'reason':reason}})
                for hidden in [SECRET,BASE.URL,'wrong-private-value','private endpoint/key detail']:
                    self.assertNotIn(hidden,output.getvalue())
                self.assertFalse(self.secret_store.values)
                self.assertEqual(self.server.events.db.execute('SELECT COUNT(*) FROM subscriptions').fetchone()[0],0)

    def test_live_admission_refuses_synthetic_store_before_side_effects(self):
        with self.assertRaises(ValueError):
            RUNTIME.configure(self.server,EVENTS,self.secret_store,callback_hosts=['callback.example.test'])
        self.assertIsNone(self.server.events)
        self.assertFalse(self.server.calls.parent.exists())

    def test_batched_watcher_does_not_starve_later_arrivals_or_ack(self):
        pump=self.configure();pump.batch=2
        self.server.dispatch(PRINCIPAL,'events/subscribe',self.subscription())
        for expected in [2,2,1,0]:self.assertEqual(len(pump.tick()),expected)
        self.assertEqual(len(self.posts),6)
        self.assertFalse(self.native.read_ids)
        self.assertFalse((self.root/'acknowledgments').exists())
        self.assertEqual(len(self.server.owned_pending()),5)
        self.assertIn('events',self.server.dispatch(PRINCIPAL,'server/discover',{})['capabilities'])

    def test_worker_reuses_existing_trigger_rearms_and_closes_on_stdio_lifecycle(self):
        pump=self.configure();pump.interval=.02
        self.server.dispatch(PRINCIPAL,'events/subscribe',self.subscription())
        used=[]
        class Trigger:
            def __init__(self,path,interval):used.append(path);self.closed=False
            def wait(self,timeout):pump.stop_event.wait(timeout)
            def close(self):self.closed=True;used.append('closed')
        with patch.object(self.server.demux,'BusEventTrigger',Trigger):
            pump.start()
            deadline=time.monotonic()+1
            while len(self.posts)<6 and time.monotonic()<deadline:time.sleep(.01)
            self.assertEqual(len(self.posts),6)
            fresh=dict(self.pending[-1],delivery_id='f'*24)
            self.state['pending'].append(fresh);self.save()
            deadline=time.monotonic()+1
            while len(self.posts)<7 and time.monotonic()<deadline:time.sleep(.01)
            self.assertEqual(len(self.posts),7)
            self.assertTrue(pump.stop())
        self.assertEqual(used,[self.root/'leases'/(self.lease+'.json'),'closed'])
        self.assertTrue(self.server.events.closed)
        self.assertEqual(pump.state,'stopped')
        with self.assertRaises(RuntimeError):pump.start()

    def test_foreign_owner_revokes_only_own_subscription_and_stops_callbacks(self):
        pump=self.configure();self.server.dispatch(PRINCIPAL,'events/subscribe',self.subscription())
        before=len(self.posts);self.state['name']='Lin';self.save()
        with self.assertRaises(PermissionError):pump.tick()
        self.assertEqual(len(self.posts),before)
        self.assertFalse(self.secret_store.values)
        self.assertEqual(self.server.events.db.execute('SELECT COUNT(*) FROM subscriptions').fetchone()[0],0)


if __name__=='__main__':unittest.main()
