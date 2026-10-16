"""Loopback fixture for tcp.php; exits after two connections."""
import socket
import threading


def reply(connection):
    with connection:
        connection.settimeout(10)
        request = bytearray()
        while chunk := connection.recv(4096):
            request.extend(chunk)
        connection.sendall(b"reply: " + request)


with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as server:
    server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    server.bind(("127.0.0.1", 8765))
    server.listen(2)
    server.settimeout(30)
    print("Listening on 127.0.0.1:8765 for two requests", flush=True)
    workers = []
    for _ in range(2):
        connection, _ = server.accept()
        worker = threading.Thread(target=reply, args=(connection,))
        worker.start()
        workers.append(worker)
    for worker in workers:
        worker.join()
