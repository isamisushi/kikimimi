import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('collector', Path(__file__).parent/'collector-plugin/scripts/collect.py')
collector = importlib.util.module_from_spec(spec)
spec.loader.exec_module(collector)

class CollectorTests(unittest.TestCase):
    def test_keyed_matching_never_exports_text(self):
        with tempfile.TemporaryDirectory() as tmp:
            path=Path(tmp)/'transcript.jsonl'
            row={'timestamp':'2026-11-01T11:08:05Z','message':{'role':'assistant','id':'msg_private','model':'claude-haiku-5-5','stop_reason':'end_turn','content':[{'type':'thinking','thinking':'PRIVATE_THOUGHT'},{'type':'text','text':'Hello\r\n世界'}],'usage':{'input_tokens':2,'output_tokens':12}}}
            path.write_text(json.dumps(row))
            data=collector.extract({'session_id':'sdk','transcript_path':str(path)},b'a'*64)
            self.assertEqual(data[0]['response_fingerprint'],'ebdad441e6740f8cb3c29d96f6b53f4ab7a9a0d1c77aefac70f5d7d033eca6b2')
            self.assertNotIn('Hello',json.dumps(data));self.assertNotIn('PRIVATE_THOUGHT',json.dumps(data))
    def test_completed_usage_deduplicated_without_content(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)/'transcript.jsonl'
            row = {'timestamp':'2026-11-01T11:08:05Z','message':{'role':'assistant','id':'msg_one','model':'claude-haiku-5-5','stop_reason':'end_turn','content':'SECRET_BODY','usage':{'input_tokens':2,'output_tokens':16,'cache_read_input_tokens':119986,'cache_creation_input_tokens':427}}}
            path.write_text(json.dumps(row)+'\n'+json.dumps(row)+'\n')
            data = collector.extract({'session_id':'session-test','transcript_path':str(path)})
            self.assertEqual(len(data),1)
            self.assertEqual(data[0]['cache_read_tokens'],119986)
            self.assertNotIn('SECRET_BODY',json.dumps(data))
            row['message']['stop_reason']=None
            path.write_text(json.dumps(row))
            self.assertEqual(collector.extract({'session_id':'session-test','transcript_path':str(path)}),[])
    def test_reconstructed_history_without_usage_not_measured(self):
        with tempfile.TemporaryDirectory() as tmp:
            path=Path(tmp)/'transcript.jsonl'
            path.write_text(json.dumps({'timestamp':'2026-11-01T11:08:05Z','message':{'role':'assistant','id':'synthetic','model':'claude-sonnet-4-6','stop_reason':'end_turn','usage':{}}}))
            self.assertEqual(collector.extract({'session_id':'test','transcript_path':str(path)}),[])

    def test_redirect_does_not_forward_credential(self):
        req=collector.urllib.request.Request('https://ingest.example/v1/claude-plugin',data=b'{}',headers={'Authorization':'Bearer test'})
        self.assertIsNone(collector.NoRedirect().redirect_request(req,None,307,'redirect',{},'https://other.example/'))

    def test_zero_usage_history_is_not_a_measured_invocation(self):
        with tempfile.TemporaryDirectory() as tmp:
            path=Path(tmp)/'transcript.jsonl'
            row={'timestamp':'2026-11-01T11:08:05Z','message':{'role':'assistant','id':'synthetic','model':'claude-sonnet-4-6','stop_reason':'end_turn','usage':{'input_tokens':0,'output_tokens':0}}}
            path.write_text(json.dumps(row))
            payload={'session_id':'test','transcript_path':str(path)}
            self.assertEqual(collector.extract(payload),[])
            row['message']['usage']['cache_read_input_tokens']=10
            path.write_text(json.dumps(row))
            self.assertEqual(len(collector.extract(payload)),1)

    def test_atomic_checkpoint_is_private(self):
        with tempfile.TemporaryDirectory() as tmp:
            path=Path(tmp)/'checkpoint.json'
            collector.atomic_json(path,['msg_one'])
            self.assertEqual(json.loads(path.read_text()),['msg_one'])
            self.assertEqual(path.stat().st_mode & 0o777,0o600)
            collector.atomic_json(path,['msg_two'])
            self.assertEqual(json.loads(path.read_text()),['msg_two'])
            self.assertEqual(len(list(Path(tmp).iterdir())),1)

if __name__=='__main__':unittest.main()
