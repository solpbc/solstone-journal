#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Exercise production audio ingestion, local completion and authorized transcript retrieval on Android."""
import argparse
import ast
import hashlib
import io
import json
import math
import pathlib
import re
import sqlite3
import struct
import subprocess
import tarfile
import time
import uuid
import wave
import zipfile
from urllib.parse import quote
from provenance import source_manifest
import qualify_serving as serving


SNAPSHOT_ROOTS = ('files/synthetic-serving-journal', 'files/worker-evidence.jsonl')


def snapshot_members(raw, expected_files):
    """Validate the closed archive namespace before any extraction."""
    if len(raw) % 512:
        raise ValueError('snapshot archive is not block aligned')
    expected_files = list(expected_files)
    if len(set(expected_files)) != len(expected_files):
        raise ValueError('duplicate source regular-file names')
    with tarfile.open(fileobj=io.BytesIO(raw)) as bundle:
        members = bundle.getmembers()
    names, files = set(), set()
    for member in members:
        name = member.name.rstrip('/')
        parts = name.split('/')
        if (not name or any(part in ('', '.', '..') for part in parts)
                or not any(name == root or name.startswith(root + '/')
                           for root in SNAPSHOT_ROOTS)
                or (name.startswith(SNAPSHOT_ROOTS[1] + '/'))):
            raise ValueError(f'unsafe snapshot member: {member.name!r}')
        if name in names:
            raise ValueError(f'duplicate snapshot member: {name!r}')
        if not (member.isfile() or member.isdir()):
            raise ValueError(f'nonregular snapshot member: {name!r}')
        names.add(name)
        if member.isfile():
            files.add(name)
    if files != set(expected_files):
        raise ValueError('snapshot regular-file population differs from stopped source')
    if SNAPSHOT_ROOTS[1] not in files:
        raise ValueError('missing regular worker evidence file')
    return members


def snapshot_file_names(raw):
    if not raw or not raw.endswith(b'\0'):
        raise ValueError('snapshot source population is empty or unterminated')
    names = [name.decode() for name in raw[:-1].split(b'\0')]
    for name in names:
        if (any(part in ('', '.', '..') for part in name.split('/'))
                or not (name.startswith(SNAPSHOT_ROOTS[0] + '/') or name == SNAPSHOT_ROOTS[1])):
            raise ValueError(f'unsafe snapshot source name: {name!r}')
    if len(set(names)) != len(names):
        raise ValueError('duplicate source regular-file names')
    if SNAPSHOT_ROOTS[1] not in names:
        raise ValueError('missing regular worker evidence file')
    return names


def capture_audio_snapshot(q):
    """Capture a stopped app's complete regular files, excluding socket entries."""
    stem = 'cache/audio-snapshot-' + uuid.uuid4().hex
    archive_name, error_name, list_name = stem + '.tar', stem + '.stderr', stem + '.list'
    witness = 'audio-snapshot-complete-' + uuid.uuid4().hex
    receipt = dict(owned_paths=[archive_name, error_name, list_name], passed=False,
                   completion_witness=witness, remote_settlement='not_started', cleanup='not_attempted')
    receipt_path = q.out / 'snapshot-capture.json'

    def save():
        receipt_path.write_text(json.dumps(receipt, indent=2) + '\n')

    settled = True

    def remote(stage, operation, *args, stdout_file=None, allow_status=False, **kwargs):
        nonlocal settled
        receipt['stage'] = stage
        save()
        try:
            result = operation(*args, check=False, **kwargs)
        except BaseException:
            settled = False
            receipt['remote_settlement'] = 'uncertain_after_' + stage
            raise
        (q.out / (stdout_file or 'snapshot-' + stage + '.stdout')).write_bytes(result.stdout)
        (q.out / ('snapshot-' + stage + '.stderr')).write_bytes(result.stderr)
        if result.returncode and not allow_status:
            # Even after tar finished, a failed adb/cat/find call is not proof
            # that this later remote command stopped. Keep owned cache paths.
            settled = False
            receipt['remote_settlement'] = 'uncertain_after_' + stage
            raise RuntimeError(f'snapshot {stage} command failed: {result.returncode}')
        return result

    save()
    try:
        before = remote('source-before', q.shell, 'run-as', serving.PACKAGE, 'find',
                        *SNAPSHOT_ROOTS, '-type', 'f', '-print0').stdout
        expected = snapshot_file_names(before)
        unsupported = remote('source-types-before', q.shell, 'run-as', serving.PACKAGE, 'find',
                             *SNAPSHOT_ROOTS, '!', '-type', 'f', '!', '-type', 'd',
                             '!', '-type', 's', '-print0').stdout
        if unsupported:
            raise ValueError('unsupported nonregular snapshot source entries')
        sockets = remote('source-sockets', q.shell, 'run-as', serving.PACKAGE, 'find',
                         *SNAPSHOT_ROOTS, '-type', 's', '-print0').stdout
        receipt['omitted_socket_paths'] = [name.decode() for name in sockets.split(b'\0') if name]
        remote('write-list', q.shell, 'run-as', serving.PACKAGE, 'sh', '-c',
               'umask 077; cat > "$1"', 'write-list', list_name, data=before)
        settled = False
        receipt['remote_settlement'] = 'creation_attempted'
        result = remote('launch', q.shell, 'run-as', serving.PACKAGE, 'sh', '-c',
                        'umask 077; tar --null -T "$3" -cf "$1" 2>"$2"; '
                        'archive_exit=$?; printf "%s:%s\\n" "$4" "$archive_exit"; exit "$archive_exit"',
                        'snapshot', archive_name, error_name, list_name, witness, allow_status=True)
        receipt['creation_controller_exit'] = result.returncode
        completion = re.fullmatch(re.escape(witness.encode()) + rb':([0-9]{1,3})\r?\n', result.stdout)
        if completion is None or int(completion[1]) > 255:
            receipt['remote_settlement'] = 'uncertain_without_completion_witness'
            raise RuntimeError('remote snapshot creation completion is uncertain')
        settled = True
        tar_exit = int(completion[1])
        receipt.update(remote_settlement='creation_completion_witness_received', actual_tar_exit=tar_exit)
        if result.returncode != tar_exit:
            settled = False
            receipt['remote_settlement'] = 'uncertain_after_controller_status_disagreement'
            raise RuntimeError('snapshot controller status differs from remote completion witness')
        errors = remote('read-stderr', q.command,
                        q.adb + ['exec-out', 'run-as', serving.PACKAGE, 'cat', error_name]).stdout
        (q.out / 'snapshot-tar.stderr').write_bytes(errors)
        # Retain available archive bytes even when tar itself failed.
        raw = remote('read-archive', q.command,
                     q.adb + ['exec-out', 'run-as', serving.PACKAGE, 'cat', archive_name],
                     stdout_file='files.tar').stdout
        receipt.update(received_bytes=len(raw), received_sha256=hashlib.sha256(raw).hexdigest())
        if tar_exit != 0:
            raise RuntimeError(f'phone snapshot tar exited {tar_exit}')
        digest = remote('read-digest', q.shell, 'run-as', serving.PACKAGE,
                        'sha256sum', archive_name).stdout.decode().split()[0]
        size = int(remote('read-size', q.shell, 'run-as', serving.PACKAGE,
                          'stat', '-Lc', '%s', archive_name).stdout)
        receipt.update(phone_sha256=digest, phone_bytes=size, received_bytes=len(raw),
                       received_sha256=hashlib.sha256(raw).hexdigest())
        save()
        if size != len(raw) or receipt['received_sha256'] != digest:
            raise ValueError('snapshot transport digest or size differs')
        after = remote('source-after', q.shell, 'run-as', serving.PACKAGE, 'find',
                       *SNAPSHOT_ROOTS, '-type', 'f', '-print0').stdout
        if set(snapshot_file_names(after)) != set(expected):
            raise ValueError('snapshot source population changed during capture')
        unsupported = remote('source-types-after', q.shell, 'run-as', serving.PACKAGE, 'find',
                             *SNAPSHOT_ROOTS, '!', '-type', 'f', '!', '-type', 'd',
                             '!', '-type', 's', '-print0').stdout
        if unsupported:
            raise ValueError('unsupported nonregular snapshot source entries')
        (q.out / 'snapshot-source-files.nul').write_bytes(after)
        snapshot_members(raw, expected)
        receipt.update(archive_verified=True, complete_regular_population=True,
                       population='equal stopped source before/after find -type f -print0 versus all regular tar members')
    except subprocess.TimeoutExpired as error:
        settled = False
        receipt.update(remote_settlement='uncertain_after_timeout', error=str(error))
        for suffix, stream in (('stdout', error.output), ('stderr', error.stderr)):
            if stream is not None:
                (q.out / ('snapshot-timeout.' + suffix)).write_bytes(
                    stream.encode() if isinstance(stream, str) else stream)
        raise
    except BaseException as error:
        receipt['error'] = str(error)
        raise
    finally:
        save()
        if settled:
            receipt['cleanup'] = 'attempted'
            save()
            try:
                result = q.shell('run-as', serving.PACKAGE, 'sh', '-c',
                                 'rm -f "$1" "$2" "$3" && test ! -e "$1" && test ! -e "$2" && test ! -e "$3"',
                                 'cleanup', archive_name, error_name, list_name, check=False)
                receipt.update(cleanup_exit=result.returncode,
                               cleanup='removed_and_absence_checked' if result.returncode == 0 else 'failed')
                if result.returncode != 0:
                    raise RuntimeError('owned snapshot archive cleanup failed')
            except BaseException as error:
                receipt.update(cleanup='uncertain' if isinstance(error, subprocess.TimeoutExpired) else 'failed',
                               cleanup_error=str(error))
                raise
            finally:
                save()
    receipt['passed'] = True
    save()


def matching_audio_failure(state, relative_input, detail):
    generation, history = state.get('generation'), state.get('recent_failures')
    if (type(generation) is not int or generation < 0 or not isinstance(history, list)
            or len(history) > 64):
        raise ValueError('audio status lacks generation-bound failure history')
    matched = None
    for failure in history:
        if (not isinstance(failure, dict) or type(failure.get('generation')) is not int
                or failure['generation'] < 0
                or not isinstance(failure.get('input'), str) or not isinstance(failure.get('error'), str)):
            raise ValueError('malformed audio failure history')
        if (failure['generation'] == generation
                and failure['input'].endswith('/' + relative_input)
                and detail in failure['error']):
            matched = failure
    return matched


def require_absent_audio_artifacts(q, base):
    for suffix in ('.jsonl', '.npz'):
        result = q.shell('run-as', serving.PACKAGE, 'sh', '-c',
                         'if [ -e "$1" ] || [ -L "$1" ]; then printf present; else printf absent; fi',
                         'artifact', base + suffix)
        if result.stdout != b'absent':
            raise ValueError(f'audio artifact exists or absence unproven: {base + suffix}')


def verify_snapshot_databases(root):
    """Check every SQLite file; search exclusion belongs to the mandatory index."""
    canonical = root / 'indexer/journal.sqlite'
    if not canonical.is_file():
        raise ValueError('missing canonical indexer/journal.sqlite')
    databases = []
    for path in sorted(root.rglob('*.sqlite')):
        db = sqlite3.connect(path.resolve().as_uri() + '?mode=ro', uri=True)
        try:
            if db.execute('pragma integrity_check').fetchall() != [('ok',)]:
                raise ValueError(f'SQLite integrity failure: {path}')
            row = dict(path=str(path.relative_to(root)), integrity='ok')
            if path == canonical:
                # Missing query tables is a failure, never an exemption.
                raw = db.execute("select count(*) from files where path like '%audio.jsonl'").fetchone()[0]
                matches = db.execute('select count(*) from chunks where chunks match ?',
                                     ('"quick brown fox"',)).fetchone()[0]
                if raw != 0 or matches != 0:
                    raise ValueError(f'raw audio search exclusion failed: files={raw}, matches={matches}')
                row.update(raw_audio_files=raw, raw_transcript_matches=matches)
            databases.append(row)
        finally:
            db.close()
    return databases


def npy_array(raw):
    assert raw[:6] == b'\x93NUMPY', 'invalid NPY magic'
    offset = 10 if raw[6] == 1 else 12
    size = int.from_bytes(raw[8:offset], 'little')
    header = ast.literal_eval(raw[offset:offset+size].decode('latin1'))
    assert not header['fortran_order'], header
    data = raw[offset+size:]
    if header['descr'].startswith('<U'):
        values = data.decode('utf-32-le').rstrip('\0')
    else:
        code = {'<f4': 'f', '<i4': 'i'}[header['descr']]
        count = math.prod(header['shape'])
        values = struct.unpack('<' + str(count) + code, data)
    return header['shape'], values


class AudioQualification(serving.Qualification):
    def __init__(self, args):
        serving.PACKAGE = args.package
        serving.ACTIVITY = args.package + '/app.solstone.development.journal.ServingActivity'
        super().__init__(args)
        self.population = []
        self.sequence = int(time.time())%2400
        self.model_held = False
        self.denied = []
        self.search_probe = None
        with wave.open(str(self.fixture('speech.wav'))) as f:
            self.speech_duration = f.getnframes() / f.getframerate()
        self.expected_words = 'the quick brown fox jumps over the lazy dog'.split()

    def fixture(self, name):
        return pathlib.Path(self.args.fixtures) / name

    def multipart(self, name, *, segment=None):
        self.sequence += 1
        length=23 if name=='long.wav' else 121 if name=='over-duration.m4a' else 3
        segment = segment or f'{int(time.time())%24:02}{self.sequence//60:02}{self.sequence%60:02}_{length}'
        raw = self.fixture(name).read_bytes()
        envelope = dict(day=self.day, segment=segment, source='audio',
                        files=[dict(submitted='audio' + pathlib.Path(name).suffix)],
                        meta=dict(platform='android-audio-development', tz='Etc/UTC', utc_offset_seconds=0))
        boundary = 'journal-audio-qualification-' + self.marker
        body = (f'--{boundary}\r\nContent-Disposition: form-data; name="envelope"\r\n\r\n'.encode()
                + json.dumps(envelope).encode()
                + f'\r\n--{boundary}\r\nContent-Disposition: form-data; name="files"; filename="{envelope["files"][0]["submitted"]}"\r\nContent-Type: application/octet-stream\r\n\r\n'.encode()
                + raw + f'\r\n--{boundary}--\r\n'.encode())
        path = self.out / f'upload-{self.sequence:03}.multipart'
        path.write_bytes(body)
        command = dict(method='POST', uri='/app/devices/ingest', body_file=str(path),
                       content_type=f'multipart/form-data; boundary={boundary}', protocol_version='3')
        item = dict(fixture=name, day=self.day, requested_segment=segment,
                    input_sha256=hashlib.sha256(raw).hexdigest(), input_bytes=len(raw), command=command,
                    duration_seconds=(121 if name=='over-duration.m4a' else 3 if name=='silence.wav' else 0 if name=='malformed.wav' else self.speech_duration * (8 if name=='long.wav' else 1)))
        return command, item

    def upload(self, name, **kwargs):
        command, item = self.multipart(name)
        start = time.monotonic()
        reply = self.network(**command, **kwargs)
        item.update(reply=reply, upload_seconds=time.monotonic()-start)
        self.population.append(item)
        self.event('upload', item=item)
        return command, item

    def input_path(self, item):
        if 'relative_input' not in item:
            segment = item['reply'].get('segment', item['reply'].get('existing_segment'))
            assert segment and re.fullmatch(r'[0-9]{6}_[0-9]+', segment), item
            directory = f'files/synthetic-serving-journal/chronicle/{item["day"]}'
            observed = self.shell('run-as', serving.PACKAGE, 'find', directory,
                                  '-type', 'd', '-name', segment).stdout.decode().splitlines()
            assert len(observed) == 1, (item, observed)
            item['relative_input'] = observed[0] + '/audio'
        return item['relative_input']

    def cat(self, path):
        p = self.shell('run-as', serving.PACKAGE, 'sh', '-c',
                       'if [ -f "$1" ]; then cat "$1"; fi', 'sh', path)
        return p.stdout or None

    def transcript_uri(self, item):
        base = pathlib.PurePosixPath(self.input_path(item))
        return '/app/transcripts/api/segment/' + '/'.join(base.parts[-4:-1])

    def retrieve(self, item):
        return self.network(method='GET', uri=self.transcript_uri(item))

    def wait_terminal(self, item, *, speech=True, timeout=240):
        start = time.monotonic()
        path = self.input_path(item) + '.jsonl'
        latencies = []
        search_latencies = []
        while time.monotonic()-start < timeout:
            before = time.monotonic()
            self.network(method='GET', uri='/api/system/status')
            if self.search_probe:
                query_started = time.monotonic()
                found = self.network(method='GET', uri=self.search_probe['uri'])
                search_latencies.append(time.monotonic()-query_started)
                assert found == self.search_probe['baseline'], (found, self.search_probe)
            latencies.append(time.monotonic()-before)
            raw = self.cat(path)
            if raw:
                rows = [json.loads(line) for line in raw.splitlines()]
                text = ' '.join(row.get('text', '') for row in rows[1:]).lower()
                if speech:
                    words = re.findall(r'[a-z]+', text)
                    expected = self.expected_words * (8 if item['fixture'] == 'long.wav' else 1)
                    distance = list(range(len(words)+1))
                    for i, word in enumerate(expected,1):
                        current=[i]
                        for j, actual in enumerate(words,1):
                            current.append(min(current[-1]+1,distance[j]+1,distance[j-1]+(word!=actual)))
                        distance=current
                    wer=distance[-1]/len(expected)
                    assert wer <= 2/9, (wer,text,rows)
                    item['word_error_rate']=wer
                    assert rows[0]['backend'] == 'parakeet-cpp', rows[0]
                    assert rows[0]['device'] == 'cpu', rows[0]
                    assert rows[0]['model'] == 'tdt-0.6b-v3-q8_0', rows[0]
                    assert rows[0].get('speaker_analysis_producer'), rows[0]
                    assert rows[0]['_solstone_processing']['state'] == 'analyzed', rows[0]
                    assert all(re.fullmatch(r'[0-9]{2}:[0-9]{2}:[0-9]{2}', row['start']) for row in rows[1:]), rows
                else:
                    assert not text, rows
                    expected = ('empty', 'no_speech') if item['fixture'] == 'silence.wav' else ('failed', 'corrupt_input')
                    proof = rows[0]['_solstone_processing']
                    assert (proof['state'], proof['reason_code']) == expected, proof
                found = self.retrieve(item)
                chunks = [chunk for chunk in found['chunks'] if chunk['type'] == 'audio']
                assert len(chunks) == len(rows)-1, (found, rows)
                for chunk, row in zip(chunks, rows[1:]):
                    assert row['text'] in chunk['markdown'], (chunk, row)
                    assert chunk['time'] == row['start'], (chunk, row)
                item['retrieved'] = found
                worker=self.control('status')
                item.pop("expected_failure", None)
                item.update(worker=worker, completion_seconds=time.monotonic()-start, rows=rows,
                            heartbeat_seconds=latencies, search_seconds=search_latencies,
                            transcript_sha256=hashlib.sha256(raw).hexdigest())
                self.event('terminal', item=item)
                return rows
            time.sleep(1)  # Test instrumentation observes response latency during inference.
        raise TimeoutError(f'no normal terminal publication for {item}; status={self.control("status")}')

    def wait_failure(self, item, detail):
        started=time.monotonic()
        relative_input = self.input_path(item) + pathlib.Path(item['fixture']).suffix
        while time.monotonic()-started<120:
            state=self.control('status')['native']['value']['audio']
            failure = matching_audio_failure(state, relative_input, detail)
            if failure is not None:
                require_absent_audio_artifacts(self, self.input_path(item))
                item.update(expected_failure=detail, failure=failure)
                self.event('bounded_failure',item=item,observed_status=state)
                return failure
            time.sleep(1)
        raise TimeoutError(f'bounded failure not observed: {item}')

    def wait_processing(self, item):
        deadline=time.monotonic()+60
        while time.monotonic()<deadline:
            state=self.control('status')['native']['value']['audio']
            if state['state']=='processing' and item['reply'].get('segment','!') in state.get('input',''):
                self.event('processing_observed',status=state)
                return
            time.sleep(.5)
        raise TimeoutError('processing never observed')

    def init(self):
        self.shell('am', 'force-stop', serving.PACKAGE)
        self.launch()
        apk_path = self.shell('pm', 'path', serving.PACKAGE).stdout.decode().strip().removeprefix('package:')
        apk = self.command(self.adb + ['exec-out', 'cat', apk_path]).stdout
        assert hashlib.sha256(apk).hexdigest() == self.args.apk_sha256
        self.event('installed_apk', sha256=self.args.apk_sha256, bytes=len(apk))
        self.event('source', manifest=source_manifest(pathlib.Path(__file__).resolve().parents[2]),
                   client_sha256=hashlib.sha256(pathlib.Path(self.args.client).read_bytes()).hexdigest(),
                   fixture_manifest=json.loads((pathlib.Path(self.args.fixtures)/'manifest.json').read_text()))
        mark = self.owner('GET', '/init/mark')
        if not mark['locked']: self.owner('POST', '/init/mark/lock', {'proposal_id': mark['proposal_id']})
        self.owner('POST', '/init/finalize', {})
        self.owner('POST', '/app/network/local-network/open', {})
        pair = self.owner('POST', '/app/network/pair-start', {'device_label': self.marker, 'role': ''})
        self.network('pair', link=pair['pair_link'], label=self.marker,
                     carrier=getattr(self.args, 'pair_carrier', 'direct'))
        if self.args.search_probe:
            uri='/app/search/api/search?q='+quote(self.args.search_probe, safe='')
            baseline=self.network(method='GET',uri=uri)
            assert baseline['total'] > 0, baseline
            self.search_probe=dict(uri=uri,baseline=baseline)
            self.event('existing_search_probe',**self.search_probe)
        self.event('fixture_contract', expected_words=self.expected_words,
                   formats=['WAV PCM16 mono 16kHz', 'AAC in M4A stereo 44.1kHz'],
                   synthetic_source='core/fixtures/parakeet_sample.wav; documented purpose-recorded phrase',
                   repeats=self.args.repeats, accuracy='normalized word error rate <= 2/9; normal transcript retrieved over authenticated LAN')
        self.sample('initial')

    def run(self):
        self.init()
        command, denied = self.multipart('speech.wav')
        for auth in ['invalid', 'missing']:
            self.network(**command, auth=auth, allow_error=True)
        self.denied.append(denied)
        command, first = self.upload('speech.wav')
        self.wait_terminal(first)
        for auth in ('invalid','missing'):
            self.network(method='GET',uri=self.transcript_uri(first),auth=auth,allow_error=True)
        assert self.network(**command)['status']=='duplicate'  # Preserve one output.
        for index in range(self.args.repeats):
            _, item = self.upload('speech.m4a' if index % 2 == 0 else 'speech.wav')
            self.wait_terminal(item)
            self.sample(f'warm-{index:03}')
        for name, detail in [('over-duration.m4a','120 seconds'),('over-size.wav','16 MiB')]:
            _, bounded=self.upload(name)
            self.wait_failure(bounded,detail)
        _, silence = self.upload('silence.wav')
        self.wait_terminal(silence, speech=False)
        _, malformed = self.upload('malformed.wav')
        self.wait_terminal(malformed, speech=False)
        command, before = self.multipart('speech.m4a')
        self.network('loss', loss='before', **command)
        directory=f'files/synthetic-serving-journal/chronicle/{before["day"]}'
        partial=self.shell('run-as',serving.PACKAGE,'find',directory,'-type','d','-name',before['requested_segment']).stdout
        assert not partial, partial
        retry = self.network(**command)
        before['reply'] = retry
        self.population.append(before)
        self.wait_terminal(before)
        command, after = self.multipart('speech.m4a')
        self.network('loss', loss='after', **command)
        retry = self.network(**command)
        assert retry['status'] == 'duplicate', retry
        after['reply'] = retry
        self.population.append(after)
        self.wait_terminal(after)
        # Stop between stages, then resume the same accepted input.
        _, cancelled = self.upload('long.wav')
        self.wait_processing(cancelled)
        self.control('audio-cancel')
        deadline=time.monotonic()+120
        while self.control('status')['native']['value']['audio']['state']!='cancelled':
            if time.monotonic()>deadline:raise TimeoutError('cancellation did not reach a stage boundary')
            time.sleep(1)
        require_absent_audio_artifacts(self, self.input_path(cancelled))
        self.control('audio-resume')
        self.wait_terminal(cancelled)
        # Recovery uses durable originals and normal app startup; no completion command.
        _, recovery = self.upload('long.wav')
        self.wait_processing(recovery)
        old = self.control('status')
        self.shell('run-as', serving.PACKAGE, 'kill', '-9', str(old['pid']))
        time.sleep(3)
        self.wait_terminal(recovery)
        new = self.control('status')
        assert new['pid'] != old['pid'] and new['uid'] == old['uid'], (old,new)
        self.event('worker_recovery', before=old, after=new)
        self.shell('am', 'force-stop', serving.PACKAGE)
        self.launch()
        retrieved = self.retrieve(first)
        assert any('quick brown fox' in chunk['markdown'].lower() for chunk in retrieved['chunks']), retrieved
        # Missing/corrupt models fail without a success artifact; repair restores work.
        model = 'files/audio-models/tdt-0.6b-v3-q8_0.gguf'
        self.shell('am', 'force-stop', serving.PACKAGE)
        self.shell('run-as', serving.PACKAGE, 'mv', model, model+'.held')
        self.model_held = True
        self.launch()
        _, held = self.upload('speech.wav')
        failed = self.wait_failure(held, 'open Parakeet model')
        assert 'open Parakeet model' in failed['error'], failed
        self.event('missing_model',status=failed)
        require_absent_audio_artifacts(self, self.input_path(held))
        self.shell('run-as', serving.PACKAGE, 'sh', '-c', 'printf broken > '+model)
        self.control('audio-resume')
        failed = self.wait_failure(held, 'size differs from pinned model')
        self.event('model_failure', status=failed)
        self.shell('run-as', serving.PACKAGE, 'mv', model+'.held', model)
        self.model_held = False
        self.control('audio-resume')
        self.wait_terminal(held)
        self.network('connect')
        self.network('held', method='GET', uri='/api/system/status')
        self.owner('POST', '/app/network/unpair', {'device_label':self.marker})
        self.network('closed')
        self.network('disconnect')
        command, revoked = self.multipart('speech.wav')
        self.network(**command, allow_error=True)
        self.denied.append(revoked)
        self.network(method='GET', uri=self.transcript_uri(first), allow_error=True)
        self.owner('POST', '/app/network/local-network/close', {})
        self.sample('finish')
        stopped = self.control('stop')
        assert stopped['native']['value'] == {'stopped': True}, stopped
        self.shell('am', 'force-stop', serving.PACKAGE)
        capture_audio_snapshot(self)
        self.verify_audio_snapshot()
        (self.out/'population.json').write_text(json.dumps(self.population,indent=2)+'\n')
        self.event('complete', passed=True, capability='local-transcription-and-authenticated-retrieval',
                   raw_transcripts_indexed=False, generated_summaries_qualified=False)

    def close(self):
        try:
            self.shell('am','force-stop',serving.PACKAGE)
            if self.model_held:
                model='files/audio-models/tdt-0.6b-v3-q8_0.gguf'
                self.shell('run-as',serving.PACKAGE,'mv',model+'.held',model)
                self.model_held=False
        finally:
            super().close()

    def verify_audio_snapshot(self):
        destination = self.out/'snapshot'
        with tarfile.open(self.out/'files.tar') as bundle: bundle.extractall(destination,filter='data')
        checks=[]
        root=destination/'files/synthetic-serving-journal'
        for item in self.population:
            base=destination/item['relative_input']
            original=base.with_suffix(pathlib.Path(item['fixture']).suffix)
            assert hashlib.sha256(original.read_bytes()).hexdigest()==item['input_sha256']
            transcript=base.with_suffix('.jsonl')
            if item.get('expected_failure'):
                assert not transcript.exists()
            else:
                assert hashlib.sha256(transcript.read_bytes()).hexdigest()==item['transcript_sha256']
                sidecar=base.with_suffix('.npz')
                if item['rows'][0]['_solstone_processing']['state'] != 'analyzed':
                    assert not sidecar.exists(), sidecar
                else:
                    assert sidecar.is_file(), sidecar
                if sidecar.is_file():
                    with zipfile.ZipFile(sidecar) as archive:
                        assert archive.testzip() is None
                        shape, embeddings = npy_array(archive.read('embeddings.npy'))
                        _, ids = npy_array(archive.read('statement_ids.npy'))
                        _, durations = npy_array(archive.read('durations_s.npy'))
                        _, encoder = npy_array(archive.read('encoder.npy'))
                        assert shape == (len(ids),256), shape
                        assert len(durations) == len(ids)
                        assert all(math.isfinite(v) for v in embeddings)
                        assert all(math.isfinite(v) and v > 0 for v in durations)
                        assert encoder == 'wespeaker-resnet34-256', encoder
                        assert set(ids) <= {row['sentence_id'] for row in item['rows'][1:]}
                        if item.get('word_error_rate') is not None: assert ids
                    item['speaker_sidecar_sha256']=hashlib.sha256(sidecar.read_bytes()).hexdigest()
            receipts=[json.loads(line) for line in (base.parent/'events.jsonl').read_text().splitlines()]
            accepted=[r for r in receipts if r.get('record_type')=='device_ingest' and r.get('outcome')=='accepted']
            assert len(accepted)==1, receipts
            assert any(f['sha256']==item['input_sha256'] for f in accepted[0]['files'])
            checks.append(dict(segment=str(base.parent.relative_to(root)),receipt=True,original=True,
                               transcript=not item.get('expected_failure'),bounded_failure=item.get('expected_failure')))
        for item in self.denied:
            assert not list((root/'chronicle'/item['day']).glob('*/'+item['requested_segment'])), item
        generated=list(root.glob('chronicle/*/*/*/talents/audio.md'))
        assert not generated, generated
        databases=verify_snapshot_databases(root)
        (self.out/'independent-verification.json').write_text(json.dumps(dict(inputs=checks,databases=databases),indent=2)+'\n')


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--adb',default='adb');p.add_argument('--serial',default='RZGL11XCS9D')
    p.add_argument('--package',default='app.solstone.development.journal.audio')
    p.add_argument('--host',required=True);p.add_argument('--port',type=int,default=7657)
    p.add_argument('--pair-carrier',choices=['direct','relay'],default='direct')
    p.add_argument('--client',required=True);p.add_argument('--apk-sha256',required=True)
    p.add_argument('--fixtures',required=True);p.add_argument('--output',required=True)
    p.add_argument('--repeats',type=int,default=10)
    p.add_argument('--search-probe',help='Existing indexed text to verify during native inference')
    args=p.parse_args()
    if args.package not in {'app.solstone.development.journal.audio', 'app.solstone.development.journal.services'}:p.error('separate audio development application required')
    q=AudioQualification(args)
    try:q.run()
    except BaseException as error:q.event('failed',error=str(error));raise
    finally:q.close()


if __name__=='__main__':main()
