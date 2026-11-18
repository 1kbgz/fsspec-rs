"""Local S3 protocol fixture shared by conditional and ranged I/O tests."""

import pytest


@pytest.fixture
def s3_endpoint():
    from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
    from threading import Thread

    objects = {}
    generations = {}
    requests = []

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, format, *args):
            pass

        def respond(self, status, body=b"", etag=None, content_range=None):
            self.send_response(status)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Last-Modified", "Tue, 17 Nov 2026 12:00:00 GMT")
            if etag is not None:
                self.send_header("ETag", etag)
            if content_range is not None:
                self.send_header("Content-Range", content_range)
            self.end_headers()
            if self.command != "HEAD":
                self.wfile.write(body)

        def do_PUT(self):
            body = self.rfile.read(int(self.headers["Content-Length"]))
            current = objects.get(self.path)
            if self.headers.get("If-None-Match") == "*" and current is not None:
                return self.respond(412)
            expected = self.headers.get("If-Match")
            if expected is not None and (current is None or current[1] != expected):
                return self.respond(412)
            generations[self.path] = generations.get(self.path, 0) + 1
            etag = f'"version-{generations[self.path]}"'
            objects[self.path] = (body, etag)
            return self.respond(200, etag=etag)

        def do_GET(self):
            requests.append((self.command, self.path, self.headers.get("Range"), self.headers.get("If-Match")))
            current = objects.get(self.path)
            if current is None:
                return self.respond(404)
            body, etag = current
            expected = self.headers.get("If-Match")
            if expected is not None and expected != etag:
                return self.respond(412)
            if self.command == "GET" and self.headers.get("Range"):
                start, end = self.headers["Range"].removeprefix("bytes=").split("-")
                start, end = int(start), int(end)
                return self.respond(206, body[start : end + 1], etag, f"bytes {start}-{end}/{len(body)}")
            return self.respond(200, body, etag)

        def do_POST(self):
            from xml.etree import ElementTree

            if not self.path.endswith("?delete"):
                return self.respond(400)
            body = self.rfile.read(int(self.headers["Content-Length"]))
            result = ElementTree.Element("DeleteResult", xmlns="http://s3.amazonaws.com/doc/2006-03-01/")
            for element in ElementTree.fromstring(body).iter():
                if element.tag.rsplit("}", 1)[-1] == "Key":
                    bucket = self.path.split("?", 1)[0]
                    objects.pop(bucket + "/" + element.text, None)
                    deleted = ElementTree.SubElement(result, "Deleted")
                    ElementTree.SubElement(deleted, "Key").text = element.text
            return self.respond(200, ElementTree.tostring(result))

        do_HEAD = do_GET

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = Thread(target=server.serve_forever, daemon=True)
    thread.start()

    class Endpoint(str):
        requests: list[tuple[str, str, str | None, str | None]]
        objects: dict[str, tuple[bytes, str | None]]
        handler: type[Handler]

    endpoint = Endpoint(f"http://127.0.0.1:{server.server_port}")
    endpoint.requests = requests
    endpoint.objects = objects
    endpoint.handler = Handler
    try:
        yield endpoint
    finally:
        server.shutdown()
        server.server_close()
        thread.join()
