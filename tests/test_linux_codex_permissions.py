"""Desktop approval buttons must resolve the same pending Codex request as the mobile Feed."""
import json
import os
from pathlib import Path
import subprocess
import tempfile

from linux_app import running_app


def main():
    """Exercise real GTK decisions, remote replies, cancellation and expiry on an owned display."""
    for key in ('CMUX_SOCKET', 'CMUX_SOCKET_PATH', 'CMUX_SURFACE_ID', 'CMUX_WORKSPACE_ID'):
        os.environ.pop(key, None)
    with tempfile.TemporaryDirectory(prefix='cm-permission-') as directory:
        with running_app(Path(directory)) as app:
            surface = next(row['uuid'] for row in app.surfaces() if row['active'])

            def windows():
                """Read only the approval windows on this fixture's isolated X server."""
                result = subprocess.run(['xdotool', 'search', '--onlyvisible', '--name',
                                         'Codex.*autorisation'], capture_output=True, text=True)
                return result.stdout.strip().splitlines()

            def push(event, wait=0):
                """Call the production control socket; keep blocking calls in their own process."""
                binary = Path(app.environment.get('CMUX_BIN_DIR', 'target/debug')) / 'cmux'
                return subprocess.Popen([str(binary), '--socket', str(app.socket_path), 'raw',
                                         'feed.push', '--params', json.dumps({'event': event,
                                         'wait_timeout_seconds': wait}), '--json'],
                                        env=app.environment, stdout=subprocess.PIPE,
                                        stderr=subprocess.PIPE, text=True)

            for index, action in enumerate(('deny', 'once', 'escape', 'mobile', 'superseded', 'timeout', 'disconnect', 'duplicate')):
                event = {'hook_event_name': 'PermissionRequest', 'session_id': f'desktop-{index}',
                         '_source': 'codex', '_opencode_request_id': f'desktop-{index}',
                         'surface_id': surface, 'cwd': directory, 'tool_name': 'Bash',
                         'tool_input': {'command': 'printf approval-fixture',
                                        'description': f'Approval fixture: {action}'}}
                process = push(event, 2 if action == 'timeout' else 10)
                try:
                    app.wait_for(windows, f'automatic visible desktop dialog ({action})', timeout=3)
                    window = windows()[-1]
                    if action == 'disconnect':
                        process.terminate(); process.wait(timeout=3)
                        app.wait_for(lambda: not windows(), 'dialog removed after client disconnection')
                        rows = json.loads(app.cli('raw', 'feed.list', '--json'))['items']
                        row = next(row for row in rows if row.get('request_id') == event['_opencode_request_id'])
                        assert row['status'] == 'expired', row
                        print('PASS desktop approval: disconnect', flush=True)
                        continue
                    if action == 'duplicate':
                        duplicate = push(dict(event, tool_input={'command': 'different command'}), 10)
                        output, error = duplicate.communicate(timeout=3)
                        assert duplicate.returncode == 0, error
                        assert json.loads(output)['accepted'] is False, output
                        assert len(windows()) == 1
                    if action in ('deny', 'once', 'escape', 'duplicate'):

                        subprocess.run(['xdotool', 'windowfocus', window], check=True)
                        key = {'deny': 'alt+r', 'once': 'alt+a', 'escape': 'Escape', 'duplicate': 'alt+r'}[action]
                        subprocess.run(['xdotool', 'key', '--clearmodifiers', key], check=True)
                    elif action == 'mobile':
                        app.cli('raw', 'feed.permission.reply', '--params',
                                json.dumps({'request_id': event['_opencode_request_id'], 'mode': 'once'}), '--json')
                    elif action == 'superseded':
                        stop = dict(event, hook_event_name='Stop')
                        done = push(stop)
                        output, error = done.communicate(timeout=3)
                        assert done.returncode == 0, error
                    output, error = process.communicate(timeout=5)
                    assert process.returncode == 0, error
                    result = json.loads(output)
                    if action in ('superseded', 'timeout'):
                        assert result['status'] in ('expired', 'timed_out'), result
                    else:
                        expected = 'once' if action in ('once', 'mobile') else 'deny'
                        assert result['status'] == 'resolved', result
                        assert result['decision']['mode'] == expected, result
                    app.wait_for(lambda: not windows(), 'dialog removed after the request ends')
                    print(f'PASS desktop approval: {action}', flush=True)
                finally:
                    if process.poll() is None:
                        process.terminate(); process.wait(timeout=3)


if __name__ == '__main__':
    main()
