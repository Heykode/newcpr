"""Offline maintenance boundaries. All tokens and identities are synthetic."""
import io
import json
import time
import types
import unittest
from http.cookiejar import Cookie
from unittest.mock import Mock, patch
from urllib.parse import parse_qs, urlparse

from relogin_worker_test import login, response, token, worker


def session_response():
    return {"accessToken": "fixture-web", "user": {"email": "test@example.invalid"}}


class MaintenanceTests(unittest.TestCase):
    def test_explicit_egress_is_kept_on_all_auth_and_maintenance_requests(self):
        for route, expected_options in (({}, {}), ({'proxy': 'http://proxy.example:8080'}, {2: ''}), ({'source_ip': '2001:db8::21'}, {1: ''})):
            session = Mock()
            session.request.return_value = types.SimpleNamespace(content=b'{}', status_code=200)
            requests = types.SimpleNamespace(Session=Mock(return_value=session))
            with patch.dict('sys.modules', {
                'curl_cffi': types.SimpleNamespace(requests=requests, CurlOpt=types.SimpleNamespace(PROXY=1, NOPROXY=2)),
                'pyotp': types.SimpleNamespace(TOTP=lambda secret: None),
            }):
                instance = worker.Login({'email': 'test@example.invalid', 'password': 'test-only-password', 'mfa_secret': 'JBSWY3DPEHPK3PXP', **route})
            self.assertEqual(requests.Session.call_args.kwargs['curl_options'], expected_options)
            for origin, path in ((worker.AUTH, '/oauth/token'), (worker.WEB, '/api/auth/session'), (worker.WEB, '/backend-api/accounts/sessions'), (worker.WEB, '/backend-api/accounts/check/v4-2023-04-27'), (worker.WEB, '/backend-api/accounts/mfa_info')):
                instance.fetch('GET', origin + path)
                self.assertEqual(session.request.call_args.kwargs['proxy'], route.get('proxy'))
                self.assertEqual(session.request.call_args.kwargs['interface'], route.get('source_ip'))

    def test_reuse_still_exchanges_new_codes_and_fresh_pkce(self):
        authorizations, exchanges, requested = [], [], []
        for index in range(2):
            instance = login()
            instance.request.update(web_session={"email": "test@example.invalid"}, action="authorize")
            auth = {"chatgpt_account_id": "team-id", "chatgpt_user_id": "user-id", "chatgpt_plan_type": "business"}
            def fetch(method, url, **kwargs):
                requested.append(url)
                if url.endswith('/api/auth/session'):
                    return response(url, session_response())
                if '/accounts/check/' in url:
                    return response(url, {"accounts": [{"id": "team-id", "plan_type": "business"}]})
                if url.endswith('/oauth/token'):
                    exchanges.append(kwargs['data'])
                    return response(url, {
                        "access_token": token({"exp": time.time() + 3600, "https://api.openai.com/auth": auth}),
                        "id_token": token({"email": "test@example.invalid", "https://api.openai.com/auth": auth}),
                        "refresh_token": "fixture-refresh",
                    })
                self.assertTrue(url.endswith('/wham/usage'))
                return response(url, {"rate_limit": {}})
            def flow(url, **kwargs):
                authorizations.append(parse_qs(urlparse(url).query))
                return f'fixture-code-{index}'
            instance.fetch, instance.flow = fetch, flow
            result = instance.run()
            self.assertTrue(result['ok'])
        self.assertEqual([x['grant_type'] for x in exchanges], ['authorization_code'] * 2)
        self.assertNotEqual(exchanges[0]['code'], exchanges[1]['code'])
        self.assertNotEqual(exchanges[0]['code_verifier'], exchanges[1]['code_verifier'])
        self.assertNotEqual(authorizations[0]['state'], authorizations[1]['state'])
        self.assertNotEqual(authorizations[0]['code_challenge'], authorizations[1]['code_challenge'])
        self.assertFalse(any('/signin/' in url for url in requested))

    def test_invalid_session_response_or_identity_does_not_trigger_login(self):
        for body in (['html'], {'unexpected': 'body'}, {'accessToken': 'fixture', 'user': {'email': 'other@example.invalid'}}):
            instance = login()
            instance.request['web_session'] = {}
            instance.request['web_session']['email'] = 'test@example.invalid'
            instance.fetch = lambda *a, **k: response(worker.WEB, body)
            instance.fresh_web_login = Mock()
            with self.assertRaises(worker.LoginError):
                instance.establish_web(allow_login=True)
            instance.fresh_web_login.assert_not_called()
        instance.fetch = lambda *a, **k: types.SimpleNamespace(status_code=200, json=Mock(side_effect=ValueError))
        with self.assertRaisesRegex(worker.LoginError, 'session_response_invalid'):
            instance.establish_web(allow_login=True)
        instance.fresh_web_login.assert_not_called()

    def test_revoked_web_backend_session_is_not_retained_or_confused_with_oauth(self):
        for path, method in (('/backend-api/accounts/mfa_info', 'GET'),
                             ('/backend-api/accounts/logout_all', 'POST'),
                             ('/backend-api/wham/usage', 'GET')):
            instance = login()
            instance.verified_session = {'email': 'test@example.invalid'}
            instance.effect_started = True
            rejected = response(worker.WEB, {'error': {'code': 'token_revoked'}})
            rejected.status_code, rejected.content = 401, b'fixture'
            instance.session.request = Mock(return_value=rejected)
            expected = 'session_expired' if '/accounts/' in path else 'upstream_rejected'
            with self.assertRaisesRegex(worker.LoginError, expected):
                instance.fetch(method, worker.WEB + path)
            if '/accounts/' in path:
                self.assertIsNone(instance.web_snapshot())
                self.assertEqual(instance.effect_started, method != 'POST')
            else:
                self.assertIsNotNone(instance.web_snapshot())

    def test_explicit_mfa_reauthentication_rejection_does_not_lock_maintenance(self):
        detail = 'User must re-authenticate to enroll/disable a factor'
        for method, path, status, message, rejected_before_effect in (
            ('POST', '/backend-api/accounts/mfa/user/disable_in_house', 401, detail, True),
            ('POST', '/backend-api/accounts/mfa/enroll', 401, detail, True),
            ('POST', '/backend-api/accounts/mfa/user/disable_in_house', 403, detail, False),
            ('POST', '/backend-api/accounts/mfa/user/disable_in_house', 401, 'unknown error', False),
            ('GET', '/backend-api/accounts/mfa_info', 401, detail, False),
        ):
            with self.subTest(method=method, path=path, status=status, message=message):
                instance = login()
                snapshot = {'email': 'test@example.invalid'}
                instance.verified_session = snapshot
                instance.effect_started = True
                rejected = response(worker.WEB, {'detail': message})
                rejected.status_code, rejected.content = status, b'fixture'
                instance.session.request = Mock(return_value=rejected)
                code = 'reauthentication_required' if rejected_before_effect else 'upstream_rejected'
                with self.assertRaisesRegex(worker.LoginError, code):
                    instance.fetch(method, worker.WEB + path)
                self.assertEqual(instance.effect_started, not rejected_before_effect)
                self.assertIs(instance.web_snapshot(), snapshot)
                self.assertEqual(instance.session.request.call_count, 1)

    def test_authorization_recovers_once_from_explicit_backend_expiry_only(self):
        for code in ('session_expired', 'upstream_rejected'):
            instance = login()
            instance.request.update(web_session={'email': 'test@example.invalid'}, action='authorize')
            instance.establish_web = Mock()
            instance.session.cookies.clear = Mock()
            instance.memberships = Mock(side_effect=[worker.LoginError(code), [{'id': 'team-id', 'plan_type': 'business'}]])
            instance.fresh_web_login = Mock()
            instance.flow = Mock(side_effect=worker.LoginError('fixture-stop'))
            with self.assertRaisesRegex(worker.LoginError, 'fixture-stop' if code == 'session_expired' else code):
                instance.run()
            self.assertEqual(instance.fresh_web_login.call_count, int(code == 'session_expired'))
            self.assertEqual(instance.memberships.call_count, 2 if code == 'session_expired' else 1)

    def test_snapshot_is_the_verified_jar_not_later_oauth_cookies(self):
        instance = login()
        cookie = Cookie(0, 'fixture', 'verified', None, False, 'chatgpt.com', False, False, '/', True, True, None, True, None, None, {}, False)
        instance.session.cookies.jar = [cookie]
        instance.fetch = lambda *a, **k: response(worker.WEB, session_response())
        instance.verify_web_session()
        cookie.value = 'unverified'
        snapshot = instance.web_snapshot()
        self.assertEqual(snapshot['cookies'][0]['value'], 'verified')
        self.assertTrue(snapshot['cookies'][0]['host_only'])

    def test_workspace_ambiguity_retains_verified_web_stage(self):
        instance = Mock()
        instance.web_snapshot.return_value = {'email': 'test@example.invalid', 'cookies': []}
        instance.run.side_effect = worker.WorkspaceSelectionRequired([{'id': 'a'}, {'id': 'b'}])
        output = io.StringIO()
        with patch.object(worker, 'Login', return_value=instance), patch.object(worker.sys, 'stdin', types.SimpleNamespace(buffer=io.BytesIO(b'{}'))), patch.object(worker.sys, 'stdout', output):
            worker.main()
        result = json.loads(output.getvalue())
        self.assertEqual(result['code'], 'workspace_ambiguous')
        self.assertEqual(result['web_session']['email'], 'test@example.invalid')
        self.assertNotIn('document', result)

    def test_totp_disabled_requires_unambiguous_flags_and_no_factors(self):
        self.assertIs(worker.totp_state({'mfa_enabled': False, 'mfa_enabled_v2': False}), False)
        self.assertIs(worker.totp_state({'mfa_enabled': True, 'factors': {'totp': [{'id': 'factor'}]}}), True)
        for info in ({}, {'mfa_enabled': False, 'mfa_enabled_v2': True}, {'mfa_enabled': False, 'factors': {'totp': [{'type': 'totp'}]}}, {'mfa_enabled': True}, {'mfa_enabled': False, 'native_default_factor_id': 'factor'}):
            self.assertIsNone(worker.totp_state(info))

    def test_totp_preparation_returns_candidate_without_replacing_current_key(self):
        instance = login()
        instance.request['action'] = 'prepare_totp'
        instance.establish_web = Mock()
        instance.security = lambda: ({}, {'totpEnabled': False})
        instance.web_api = Mock(return_value=response(worker.WEB, {
            'secret': 'GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ',
            'session_id': 'fixture-enrollment',
        }))
        result = instance.run()
        self.assertEqual(result['pending_totp']['session_id'], 'fixture-enrollment')
        self.assertTrue(result['view']['pendingTotp'])
        self.assertNotIn('new_secret', result)
        instance.web_api.assert_called_once_with('POST', '/accounts/mfa/enroll', {'factor_type': 'totp'})

    def test_totp_activation_requires_ack_for_the_saved_enrollment(self):
        secret = 'GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ'
        for ack in ({'success': True}, {'success': False}, {'mfa_enabled': True}):
            instance = login()
            instance.request.update(action='activate_totp', pending_totp={
                'secret': secret, 'session_id': 'fixture-enrollment',
            })
            instance.establish_web = Mock()
            instance.web_api = Mock(return_value=response(worker.WEB, ack))
            otp = types.SimpleNamespace(TOTP=lambda key: types.SimpleNamespace(now=lambda: '123456'))
            with patch.dict(worker.sys.modules, {'pyotp': otp}), patch.object(worker.time, 'time', return_value=60):
                if ack.get('success') is True:
                    result = instance.run()
                    self.assertEqual(result['new_secret'], secret)
                    self.assertFalse(result['view']['pendingTotp'])
                else:
                    with self.assertRaisesRegex(worker.LoginError, 'totp_activation_unconfirmed'):
                        instance.run()
                    self.assertTrue(instance.effect_started)
            instance.web_api.assert_called_once_with('POST', '/accounts/mfa/user/activate_enrollment', {
                'code': '123456', 'factor_type': 'totp', 'session_id': 'fixture-enrollment',
            })

    def test_totp_disable_confirms_state_after_message_only_response(self):
        for disabled in (True, False):
            instance = login()
            instance.request['action'] = 'disable_totp'
            instance.establish_web = Mock()
            instance.security = Mock(side_effect=[
                ({'factors': {'totp': [{'id': 'fixture-factor'}]}}, {'totpEnabled': True}),
                ({}, {'totpEnabled': False if disabled else None}),
            ])
            instance.web_api = Mock(return_value=response(worker.WEB, {
                'message': 'fixture acknowledgement', 'passkey_credential_manager_sync': {},
            }))
            if disabled:
                self.assertFalse(instance.run()['view']['totpEnabled'])
            else:
                with self.assertRaisesRegex(worker.LoginError, 'totp_disable_unconfirmed'):
                    instance.run()
            self.assertEqual(instance.security.call_count, 2)
            instance.web_api.assert_called_once_with('POST', '/accounts/mfa/user/disable_in_house', {
                'factor_id': 'fixture-factor',
            })

    def test_logout_never_accepts_redirect_html_or_negative_ack(self):
        invalid = [response(worker.WEB, location='/log-in'), response(worker.WEB, {'ok': False}), response(worker.WEB, {'success': False}), response(worker.WEB, {'error': 'failed'}), types.SimpleNamespace(status_code=200, json=Mock(side_effect=ValueError))]
        for item in invalid:
            with self.assertRaisesRegex(worker.LoginError, 'operation_unconfirmed'):
                worker.acknowledged(item)
        worker.acknowledged(types.SimpleNamespace(status_code=204))
        worker.acknowledged(response(worker.WEB, {'success': True}))

    def test_logout_all_accepts_json_null_without_replaying_or_claiming_immediate_revocation(self):
        for action in ('logout_all', 'logout_and_relogin'):
            instance = login()
            instance.request['action'] = action
            instance.establish_web = Mock()
            instance.verified_session = {'email': 'test@example.invalid'}
            instance.web_api = Mock(return_value=types.SimpleNamespace(status_code=200, json=lambda: None))
            result = instance.run()
            self.assertTrue(result['ok'])
            self.assertIsNone(result['web_session'])
            self.assertEqual(result['view']['sessionState'], 'absent')
            self.assertIn('延迟', result['view']['message'])
            instance.web_api.assert_called_once_with('POST', '/accounts/logout_all', {})

    def test_web_navigation_skips_denied_space_but_oauth_keeps_target(self):
        for oauth in (False, True):
            instance = login()
            instance.request['workspace_id'] = 'team-id'
            instance.selected = {'id': 'team-id'} if oauth else None
            page = response(worker.AUTH, {'page': {'type': 'workspace', 'payload': {'current_workspace_id': 'team-id', 'workspaces': [{'id': 'team-id'}, {'id': 'free-id'}]}}})
            calls = []
            def fetch(method, url, **kwargs):
                if method == 'GET':
                    return page
                calls.append(kwargs['json']['workspace_id'])
                if calls[-1] == 'team-id':
                    raise worker.LoginError('workspace_unavailable')
                return response(worker.WEB)
            instance.fetch = fetch
            if oauth:
                with self.assertRaisesRegex(worker.LoginError, 'workspace_unavailable'):
                    instance.flow(worker.AUTH, oauth=True, state='fixture')
                self.assertEqual(calls, ['team-id'])
            else:
                instance.flow(worker.AUTH)
                self.assertEqual(calls, ['team-id', 'free-id'])

    def test_disabled_membership_alias_cannot_win_highest_plan(self):
        items = [{'id': 'team', 'plan_type': 'team', 'can_access_with_session': False}, {'id': 'team', 'plan_type': 'business'}, {'id': 'free', 'plan_type': 'free'}]
        self.assertEqual(worker.select_workspace(items)['id'], 'free')
        with self.assertRaisesRegex(worker.LoginError, 'workspace_unavailable'):
            worker.select_workspace(items, 'team')

    def test_http_402_workspace_denial_is_distinct_from_missing_directory(self):
        instance = login()
        rejected = response(worker.AUTH, {'error': {'code': 'deactivated_workspace'}})
        rejected.status_code, rejected.content = 402, b'fixture'
        instance.session.request = lambda *a, **k: rejected
        with self.assertRaisesRegex(worker.LoginError, 'workspace_unavailable'):
            instance.fetch('POST', worker.AUTH + '/api/accounts/workspace/select')
        with self.assertRaisesRegex(worker.LoginError, 'workspace_missing'):
            worker.select_workspace([{'id': 'free', 'plan_type': 'free'}], 'team')
