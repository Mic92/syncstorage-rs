# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this file,
# You can obtain one at http://mozilla.org/MPL/2.0/.
from base64 import urlsafe_b64decode
import binascii
import hashlib
import hmac
import json
import jwt
import random
import string
import time
import tokenlib
import unittest

from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric import rsa
from cryptography.hazmat.backends import default_backend
from fxa.tools.bearer import get_bearer_token
from fxa.core import Client
from fxa.oauth import Client as OAuthClient
from fxa.tests.utils import TestEmailAccount
from hashlib import sha256
from tokenlib import HKDF

from tokenserver.test_support import TestCase

# This is the client ID used for Firefox Desktop. The FxA team confirmed that
# this is the proper client ID to be using for these integration tests.
CLIENT_ID = '5882386c6d801776'
DEFAULT_TOKEN_DURATION = 300
FXA_ACCOUNT_STAGE_HOST = 'https://api-accounts.stage.mozaws.net'
FXA_OAUTH_STAGE_HOST = 'https://oauth.stage.mozaws.net'
PASSWORD_CHARACTERS = string.ascii_letters + string.punctuation + string.digits
PASSWORD_LENGTH = 32
SCOPE = 'https://identity.mozilla.com/apps/oldsync'


class TestE2e(TestCase, unittest.TestCase):

    def setUp(self):
        super(TestE2e, self).setUp()

    def tearDown(self):
        super(TestE2e, self).tearDown()

    @classmethod
    def setUpClass(cls):
        # Create an ephemeral email account to use to create an FxA account
        cls.acct = TestEmailAccount()
        cls.client = Client(FXA_ACCOUNT_STAGE_HOST)
        cls.oauth_client = OAuthClient(CLIENT_ID, None,
                                       server_url=FXA_OAUTH_STAGE_HOST)
        cls.fxa_password = cls._generate_password()
        # Create an FxA account for these end-to-end tests
        cls.session = cls.client.create_account(cls.acct.email,
                                                password=cls.fxa_password)
        # Loop until we receive the verification email from FxA
        while not cls.acct.messages:
            time.sleep(0.5)
            cls.acct.fetch()
        # Find the message containing the verification code and verify the
        # code
        for m in cls.acct.messages:
            if 'x-verify-code' in m['headers']:
                cls.session.verify_email_code(m['headers']['x-verify-code'])
        # Create an OAuth token to be used for the end-to-end tests
        cls.oauth_token = cls.oauth_client.authorize_token(cls.session, SCOPE)

    @classmethod
    def tearDownClass(cls):
        cls.acct.clear()
        cls.client.destroy_account(cls.acct.email, cls.fxa_password)

    @staticmethod
    def _generate_password():
        r = range(PASSWORD_LENGTH)

        return ''.join(random.choice(PASSWORD_CHARACTERS) for i in r)

    def _get_token_with_bad_scope(self):
        bad_scope = 'bad_scope'

        return get_bearer_token(TestE2e.acct.email,
                                TestE2e.fxa_password,
                                scopes=[bad_scope],
                                account_server_url=FXA_ACCOUNT_STAGE_HOST,
                                oauth_server_url=FXA_OAUTH_STAGE_HOST,
                                client_id=CLIENT_ID)

    def _get_bad_token(self):
        key = rsa.generate_private_key(backend=default_backend(),
                                       public_exponent=65537,
                                       key_size=2048)
        format = serialization.PrivateFormat.TraditionalOpenSSL
        algorithm = serialization.NoEncryption()
        pem = key.private_bytes(encoding=serialization.Encoding.PEM,
                                format=format,
                                encryption_algorithm=algorithm)
        private_key = pem.decode('utf-8')
        claims = {
            'sub': 'fake sub',
            'iat': 12345,
            'exp': 12345,
        }

        return jwt.encode(claims, private_key, algorithm='RS256')

    @classmethod
    def _change_password(cls):
        new_password = cls._generate_password()
        cls.session.change_password(cls.fxa_password, new_password)
        cls.fxa_password = new_password
        # Refresh the session
        cls.session = cls.client.login(cls.acct.email, cls.fxa_password)
        # Refresh the OAuth token
        cls.oauth_token = cls.oauth_client.authorize_token(cls.session, SCOPE)

    # Adapted from the original Tokenserver:
    # https://github.com/mozilla-services/tokenserver/blob/master/tokenserver/util.py#L24
    def _fxa_metrics_hash(self, value):
        hasher = hmac.new(self.FXA_METRICS_HASH_SECRET.encode('utf-8'), b'',
                          sha256)
        hasher.update(value.encode('utf-8'))
        return hasher.hexdigest()

    def _derive_secret(self, master_secret):
        info = "services.mozilla.com/mozsvc/v1/node_secret/%s" % self.NODE_URL
        hkdf_params = {
            "salt": None,
            "info": info.encode("utf-8"),
            "hashmod": hashlib.sha256,
        }
        size = len(master_secret) // 2
        derived_secret = HKDF(master_secret.encode("utf-8"), size=size,
                              **hkdf_params)

        return binascii.b2a_hex(derived_secret).decode()

    def test_unauthorized_error_status(self):
        # Totally busted auth -> generic error.
        headers = {
            'Authorization': 'Unsupported-Auth-Scheme IHACKYOU',
            'X-KeyID': '1234-YWFh'
        }
        res = self.app.get('/1.0/sync/1.5', headers=headers, status=401)
        expected_error_response = {
            'errors': [
                {
                    'description': 'Unsupported',
                    'location': 'body',
                    'name': ''
                }
            ],
            'status': 'error'
        }
        self.assertEqual(res.json, expected_error_response)
        token = self._get_bad_token()
        headers = {
            'Authorization': 'Bearer %s' % token,
            'X-KeyID': '1234-YWFh'
        }
        # Bad token -> 'invalid-credentials'
        res = self.app.get('/1.0/sync/1.5', headers=headers, status=401)
        expected_error_response = {
            'errors': [
                {
                    'description': 'Unauthorized',
                    'location': 'body',
                    'name': ''
                }
            ],
            'status': 'invalid-credentials'
        }
        self.assertEqual(res.json, expected_error_response)
        # Untrusted scopes -> 'invalid-credentials'
        token = self._get_token_with_bad_scope()
        headers = {
            'Authorization': 'Bearer %s' % token,
            'X-KeyID': '1234-YWFh'
        }
        res = self.app.get('/1.0/sync/1.5', headers=headers, status=401)
        self.assertEqual(res.json, expected_error_response)

    def test_valid_request(self):
        oauth_token = self.oauth_token
        headers = {
            'Authorization': 'Bearer %s' % oauth_token,
            'X-KeyID': '1234-YWFh'
        }
        # Send a valid request, allocating a new user
        res = self.app.get('/1.0/sync/1.5', headers=headers)
        fxa_uid = self.session.uid
        # Retrieve the user from the database
        user = self._get_user(res.json['uid'])
        # First, let's verify that the token we received is valid. To do this,
        # we can unpack the hawk header ID into the payload and its signature
        # and then construct a tokenlib token to compute the signature
        # ourselves. To obtain a matching signature, we use the same secret as
        # is used by Tokenserver.
        raw = urlsafe_b64decode(res.json['id'])
        payload = raw[:-32]
        signature = raw[-32:]
        payload_dict = json.loads(payload.decode('utf-8'))

        signing_secret = binascii.b2a_hex(
            self.TOKEN_SIGNING_SECRET.encode("utf-8")).decode()
        node_specific_secret = self._derive_secret(signing_secret)
        expected_token = tokenlib.make_token(payload_dict,
                                             secret=node_specific_secret)
        expected_signature = urlsafe_b64decode(expected_token)[-32:]
        # Using the #compare_digest method here is not strictly necessary, as
        # this is not a security-sensitive situation, but it's good practice
        self.assertTrue(hmac.compare_digest(expected_signature, signature))
        # Check that the given key is a secret derived from the hawk ID
        expected_secret = tokenlib.get_derived_secret(
            res.json['id'], secret=node_specific_secret)
        self.assertEqual(res.json['key'], expected_secret)
        # Check to make sure the remainder of the fields are valid
        self.assertEqual(res.json['uid'], user['uid'])
        self.assertEqual(res.json['api_endpoint'],
                         '%s/1.5/%s' % (self.NODE_URL, user['uid']))
        self.assertEqual(res.json['duration'], DEFAULT_TOKEN_DURATION)
        self.assertEqual(res.json['hashalg'], 'sha256')
        self.assertEqual(res.json['hashed_fxa_uid'],
                         self._fxa_metrics_hash(fxa_uid)[:32])
        self.assertEqual(res.json['node_type'], 'spanner')