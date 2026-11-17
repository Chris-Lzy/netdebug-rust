import socket, threading, sys

srv = socket.socket()
srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
port = int(sys.argv[1]) if len(sys.argv) > 1 else 5020
srv.bind(("127.0.0.1", port))
srv.listen(4)
print(f"fake slave on {port}", flush=True)
tick = 0

def handle(c):
    global tick
    try:
        while True:
            req = c.recv(256)
            if not req:
                break
            fc = req[7]
            if fc == 3:
                tick += 1
                regs = b"".join(((100 + i + tick) % 65536).to_bytes(2, "big") for i in range(10))
                pdu = bytes([3, len(regs)]) + regs
            else:
                pdu = bytes([fc | 0x80, 1])
            resp = req[0:2] + b"\x00\x00" + (len(pdu) + 1).to_bytes(2, "big") + req[6:7] + pdu
            c.sendall(resp)
    except OSError:
        pass
    finally:
        c.close()

while True:
    c, _ = srv.accept()
    threading.Thread(target=handle, args=(c,), daemon=True).start()