"""Pure fault barriers complement, and never replace, actual Linux kernel acceptance."""
import copy
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec=importlib.util.spec_from_file_location('bootstrap',Path(__file__).with_name('bootstrap-data-volume.py'))
b=importlib.util.module_from_spec(spec); spec.loader.exec_module(b)
BASE=json.loads(json.loads(Path(__file__).with_name('guest-bootstrap-fixtures.json').read_text())['cases'][0]['json'])

class Host:
    def __init__(self):
        self.size=80
        self.disk={'path':'/dev/owned-test-only','type':'disk','size':80,'serial':BASE['volume_id'].replace('-',''),'model':'Amazon Elastic Block Store','maj:min':'8:16','uuid':None}
        self.devices=[self.disk]
        self.effects=[]; self.used=False; self.root=False; self.blank_ok=True; self.filesystem=False; self.layout_ok=True;self.fail_format=False;self.after_format=False
    def probe(self): return copy.deepcopy(self.devices)
    def check_use(self,d,formatting=False):
        b.require(not self.root,'disk_identity_mismatch');b.require(not self.used,'disk_in_use')
    def blank(self,d): b.require(self.blank_ok,'disk_not_blank')
    def format_once(self,d):
        self.effects.append('format')
        if self.fail_format: raise b.Hold('disk_format_uncertain')
        self.filesystem=True
        if self.after_format: raise b.Hold('disk_format_uncertain')
    def verify_filesystem(self,d): b.require(self.filesystem,'disk_format_uncertain')
    def verify_complete_format(self,d): b.require(self.filesystem,'disk_format_uncertain')
    def layout(self,d,resume_new):
        b.require(self.layout_ok,'lineage_mismatch');self.effects.append('layout')
    def verify_layout(self,d): b.require(self.layout_ok,'layout_unavailable')

class BootstrapBarriers(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory(prefix='craxii-bootstrap-unit-')
        self.root=Path(self.temp.name);self.host=Host();self.c=copy.deepcopy(BASE);self.events=[]
        # Explicit unit-test receipt injection; installed CLI exposes no such selection.
        def record(path,material,**kwargs):
            self.events.append(material['phase'])
            path.write_bytes(b.canonical(material))
        self.writer=patch.object(b,'atomic_record',side_effect=record);self.writer.start()
        self.reader=patch.object(b,'read_safe',side_effect=lambda path,*args,**kwargs:Path(path).read_bytes());self.reader.start()
    def tearDown(self): self.reader.stop();self.writer.stop();self.temp.cleanup()
    def engine(self):return b.Bootstrap(self.host,self.c,self.root)
    def test_exact_selection_ignores_equal_sized_foreign_disk(self):
        foreign=copy.deepcopy(self.host.disk);foreign.update(serial='vol00000000000000002',**{'maj:min':'8:32'})
        self.host.devices.insert(0,foreign)
        self.assertEqual(b.select(self.host,self.c)['maj:min'],'8:16')
    def test_refusals_have_no_format_or_receipt(self):
        for changed in ('identity','size','model','ambiguous','root','used','nonzero','clone'):
            with self.subTest(changed=changed):
                h=Host()
                if changed=='identity':h.disk['serial']='vol00000000000000002'
                if changed=='size':h.disk['size']=81
                if changed=='model':h.disk['model']='foreign'
                if changed=='ambiguous':h.devices.append(copy.deepcopy(h.disk))
                if changed=='root':h.root=True
                if changed=='used':h.used=True
                if changed=='nonzero':h.blank_ok=False
                if changed=='clone':
                    foreign=copy.deepcopy(h.disk);foreign.update(serial='vol00000000000000002',uuid=self.c['filesystem_uuid'],**{'maj:min':'8:32'});h.devices.append(foreign)
                with self.assertRaises(b.Hold):b.Bootstrap(h,self.c,self.root).run()
                self.assertEqual(h.effects,[]);self.assertEqual(self.events,[])
    def test_intent_commits_before_only_format(self):
        original=self.host.format_once
        def format_once(d):
            self.assertEqual(self.events,['format_intended']);self.assertEqual(b.decode(next(self.root.glob('*.json')).read_bytes())['phase'],'format_intended');original(d)
        self.host.format_once=format_once
        self.assertEqual(self.engine().run(),'layout_verified');self.assertEqual(self.host.effects,['format','layout'])
    def test_intent_persistence_failure_never_formats(self):
        self.writer.stop();self.writer=patch.object(b,'atomic_record',side_effect=OSError('synthetic write failure'));self.writer.start()
        with self.assertRaises(OSError):self.engine().run()
        self.assertEqual(self.host.effects,[])
    def test_unknown_format_blank_restart_never_retries(self):
        self.host.fail_format=True
        with self.assertRaises(b.Hold):self.engine().run()
        self.host.fail_format=False
        with self.assertRaises(b.Hold):self.engine().run()
        self.assertEqual(self.host.effects,['format'])
    def test_completed_format_crash_resumes_checks_only(self):
        self.host.after_format=True
        with self.assertRaises(b.Hold):self.engine().run()
        self.host.after_format=False
        self.assertEqual(self.engine().run(),'layout_verified');self.assertEqual(self.host.effects,['format','layout'])
    def test_layout_crash_and_immutable_config_conflict(self):
        self.host.layout_ok=False
        with self.assertRaises(b.Hold):self.engine().run()
        self.c['generation']='2';self.host.layout_ok=True
        with self.assertRaises(b.Hold):self.engine().run()
        self.assertEqual(self.host.effects,['format'])
    def test_adoption_modes_have_no_format_path(self):
        for mode in ('adopt_existing','adopt_restored'):
            with self.subTest(mode=mode):
                with tempfile.TemporaryDirectory() as root:
                    self.c['mode']=mode;self.host.filesystem=True
                    self.assertEqual(b.Bootstrap(self.host,self.c,root).run(),'layout_verified')
        self.assertEqual(self.host.effects,['layout','layout'])
    def test_conflicting_adoption_holds(self):
        self.c['mode']='adopt_existing';self.host.filesystem=True;self.host.layout_ok=False
        with self.assertRaises(b.Hold):self.engine().run()
        self.assertEqual(self.host.effects,[])
    def test_duplicate_receipt_material_is_rejected(self):
        with self.assertRaises(b.Hold):b.decode(b'{"phase":1,"phase":2}')

if __name__=='__main__':unittest.main()
