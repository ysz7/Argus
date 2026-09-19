#!/usr/bin/env python3
"""Follow an application's interface through a running `argus serve`.

A client of the Argus local service that uses only the Python standard
library: it takes one full observation, then asks for the changes since the
last one.

    argus serve &
    python3 examples/python/watch.py --app Calculator --count 5

Prints a summary line per observation (element counts, not screen content)
unless --verbose is given.
"""

import argparse
import json
import sys
import time
import urllib.error
import urllib.parse
import urllib.request


class ArgusError(Exception):
    """An error answer of the service: {"error": {"code": ..., "message": ...}}."""

    def __init__(self, status, error):
        super().__init__(f"{status} {error.get('code')}: {error.get('message')}")
        self.status = status
        self.code = error.get("code")
        self.error = error


class Argus:
    """A minimal client of the Argus local service."""

    def __init__(self, url="http://127.0.0.1:7412"):
        self.url = url.rstrip("/")

    def get(self, path, **params):
        query = urllib.parse.urlencode({k: v for k, v in params.items() if v is not None})
        url = f"{self.url}{path}" + (f"?{query}" if query else "")
        try:
            with urllib.request.urlopen(url, timeout=60) as response:
                return json.load(response)
        except urllib.error.HTTPError as error:
            body = json.load(error)
            raise ArgusError(error.code, body.get("error", {})) from None

    def health(self):
        return self.get("/v1/health")

    def observation(self, app=None, pid=None, sources=None):
        return self.get("/v1/observation", app=app, pid=pid, sources=sources)

    def stored(self, observation_id):
        return self.get(f"/v1/observation/{urllib.parse.quote(observation_id)}")

    def changes(self, since):
        return self.get("/v1/changes", since=since)

    def element(self, element_id, observation=None):
        return self.get(f"/v1/elements/{urllib.parse.quote(element_id)}", observation=observation)


def summary(observation):
    app = (observation.get("application") or {}).get("name") or "?"
    interactive = {"button", "text_box", "checkbox", "radio_button", "menu_item", "tab",
                   "list_item", "slider", "link"}
    elements = observation["elements"]
    count = sum(1 for element in elements if element["role"] in interactive)
    return f"{observation['id']}  {app}: {len(elements)} elements, {count} interactive"


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--url", default="http://127.0.0.1:7412")
    parser.add_argument("--app", help="application name or bundle id (default: frontmost)")
    parser.add_argument("--sources", help="e.g. accessibility,ocr,vision (default: all)")
    parser.add_argument("--count", type=int, default=5, help="observations to make")
    parser.add_argument("--interval", type=float, default=1.0, help="seconds between them")
    parser.add_argument("--verbose", action="store_true", help="print the JSON answers")
    args = parser.parse_args()

    argus = Argus(args.url)
    try:
        health = argus.health()
    except urllib.error.URLError as error:
        sys.exit(f"Argus is not running at {args.url} ({error.reason}); start `argus serve`")
    print(f"argus {health['version']}, protocol {health['protocol_version']}")

    observation = argus.observation(app=args.app, sources=args.sources)
    print(summary(observation))
    if args.verbose:
        print(json.dumps(observation, indent=2, ensure_ascii=False))
    for _ in range(args.count - 1):
        time.sleep(args.interval)
        try:
            delta = argus.changes(observation["id"])
        except ArgusError as error:
            if error.code != "session_changed":
                raise
            # Another application or window: start over from a full observation.
            observation = argus.stored(error.error["observation"])
            print(f"new session  {summary(observation)}")
            continue
        observation = {"id": delta["to"], "application": observation.get("application")}
        # Confidence changes are not interface changes (the second observation
        # of a session gains `confidence.identity` everywhere).
        changed = {change["id"] for change in delta["changed"]
                   if not change["property"].startswith("confidence.")}
        print(f"{delta['to']}  +{len(delta['added'])} added, -{len(delta['removed'])} removed, "
              f"{len(changed)} changed")
        if args.verbose:
            print(json.dumps(delta, indent=2, ensure_ascii=False))


if __name__ == "__main__":
    try:
        main()
    except ArgusError as error:
        sys.exit(f"argus: {error}")
