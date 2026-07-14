"""A minimal MCP/JSON-RPC client over the kernel's framed serial (P4, M8).

Inbound requests use the *same* length-prefixed frame as outbound events
(`framing.encode_frame`): the serial layer stays dumb, JSON-RPC rides inside.
Responses arrive wrapped in the kernel's stream envelope
`{"id":<stream>,"type":"rpc","rpc":{…}}`; this client unwraps `.rpc` and
correlates on the JSON-RPC `id`, passing every intervening event frame
(ticks, payload events) to an optional `collect` sink so callers can assert
on the async event stream a tool call kicks off.
"""

import json

from .framing import encode_frame


class Mcp:
    def __init__(self, qemu):
        self.q = qemu
        self._next_id = 1

    def send(self, method, params=None, req_id=None):
        """Fire a request without waiting; returns the request id used."""
        if req_id is None:
            req_id = self._next_id
            self._next_id += 1
        req = {"jsonrpc": "2.0", "id": req_id, "method": method}
        if params is not None:
            req["params"] = params
        self.q.send(encode_frame(json.dumps(req).encode()))
        return req_id

    def call(self, method, params=None, req_id=None, timeout=60, collect=None):
        """Send a request and return the correlated JSON-RPC response object.

        Every frame seen before the response (ticks, payload events from an
        async tool) is appended to `collect` if given. Raises on EOF."""
        rid = self.send(method, params, req_id)
        while True:
            frame = self.q.stream.next_frame(timeout)
            if frame is None:
                raise AssertionError(f"EOF waiting for rpc response to {method!r}")
            evt = json.loads(frame)
            if collect is not None:
                collect.append(evt)
            if evt.get("type") == "rpc" and evt.get("rpc", {}).get("id") == rid:
                return evt["rpc"]

    def result(self, method, params=None, **kw):
        """Like `call`, but assert success and return the `result` payload."""
        resp = self.call(method, params, **kw)
        assert "result" in resp, f"{method} returned an error: {resp}"
        return resp["result"]
