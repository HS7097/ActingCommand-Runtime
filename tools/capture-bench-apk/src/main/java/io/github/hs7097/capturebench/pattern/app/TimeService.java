package io.github.hs7097.capturebench.pattern.app;

import android.net.LocalServerSocket;
import android.net.LocalSocket;
import android.util.Log;

import io.github.hs7097.capturebench.pattern.core.Command;
import io.github.hs7097.capturebench.pattern.core.Json;
import io.github.hs7097.capturebench.pattern.core.LineReader;
import io.github.hs7097.capturebench.pattern.core.ProtocolError;
import io.github.hs7097.capturebench.pattern.core.ProtocolInfo;

import java.io.IOException;
import java.io.OutputStream;
import java.nio.charset.StandardCharsets;
import java.util.concurrent.atomic.AtomicInteger;

/**
 * The time service (PROTOCOL.md, "Wire protocol"): an abstract-namespace socket the bench reaches
 * with {@code adb forward tcp:<port> localabstract:capture_bench_pattern}. One thread per
 * connection; requests on a connection are answered strictly in order, one line each.
 * It needs no permission and opens no network socket.
 */
final class TimeService {
    static final int MAX_CONNECTIONS = 8;

    private final Bench bench;
    private final AtomicInteger open = new AtomicInteger();
    private volatile String error;

    TimeService(Bench bench) {
        this.bench = bench;
    }

    /** The bind or accept failure that stopped the service, or null while it serves. */
    String error() {
        return error;
    }

    void start() {
        Thread acceptor = new Thread(new Runnable() {
            @Override
            public void run() {
                acceptLoop();
            }
        }, "time-service");
        acceptor.setDaemon(true);
        acceptor.start();
    }

    private void acceptLoop() {
        LocalServerSocket server;
        try {
            server = new LocalServerSocket(ProtocolInfo.SOCKET_NAME);
        } catch (IOException e) {
            error = "bind " + ProtocolInfo.SOCKET_NAME + ": " + e;
            bench.error("server_bind_failed", error);
            return;
        }
        bench.book.event(Json.obj().put("type", "server").put("event", "listening")
                .put("socket", ProtocolInfo.SOCKET_NAME).put("t_dev_ns", System.nanoTime()));
        while (true) {
            LocalSocket socket;
            try {
                socket = server.accept();
            } catch (IOException e) {
                // The service stops here and says so; the bench sees refused connections and HELLO is gone.
                error = "accept: " + e;
                bench.error("server_accept_failed", error);
                return;
            }
            if (open.incrementAndGet() > MAX_CONNECTIONS) {
                open.decrementAndGet();
                refuse(socket, new ProtocolError("too_many_connections", "at most " + MAX_CONNECTIONS
                        + " connections are served at once"));
                continue;
            }
            final LocalSocket accepted = socket;
            Thread connection = new Thread(new Runnable() {
                @Override
                public void run() {
                    try {
                        serve(accepted);
                    } finally {
                        open.decrementAndGet();
                        close(accepted);
                    }
                }
            }, "time-service-connection");
            connection.setDaemon(true);
            connection.start();
        }
    }

    private void refuse(LocalSocket socket, ProtocolError e) {
        try {
            write(socket.getOutputStream(), e.toJson(null));
        } catch (IOException io) {
            Log.w(Bench.TAG, "could not send " + e.code + " to a refused connection", io);
        } finally {
            close(socket);
        }
    }

    private void serve(LocalSocket socket) {
        OutputStream out;
        try {
            out = socket.getOutputStream();
            int uid = socket.getPeerCredentials().getUid();
            if (!Bench.peerAllowed(uid)) {
                bench.error("peer_not_allowed", "a connection from uid " + uid + " was refused");
                write(out, new ProtocolError("peer_not_allowed", "uid " + uid + " may not use this service").toJson(null)
                        .put("uid", uid));
                return;
            }
            LineReader in = new LineReader(socket.getInputStream());
            while (true) {
                String line;
                try {
                    line = in.readLine(Command.MAX_LINE_BYTES);
                } catch (ProtocolError e) {
                    write(out, e.toJson(null));
                    return;
                }
                if (line == null) {
                    return;
                }
                long tRx = System.nanoTime();
                Command c;
                try {
                    c = Command.parse(line);
                } catch (ProtocolError e) {
                    write(out, e.toJson(Command.verbOf(line)));
                    if (e.closeConnection) {
                        return;
                    }
                    continue;
                }
                if ("PING".equals(c.verb)) {
                    // Answered here, with nothing between the clock reads and the write.
                    String head = "{\"ok\":true,\"cmd\":\"PING\",\"token\":" + Json.quote(c.token) + ",\"t_rx_dev_ns\":"
                            + tRx + ",\"t_tx_dev_ns\":";
                    long tTx = System.nanoTime();
                    out.write((head + tTx + "}\n").getBytes(StandardCharsets.UTF_8));
                    out.flush();
                    continue;
                }
                if ("BYE".equals(c.verb)) {
                    write(out, Bench.ok("BYE"));
                    return;
                }
                Json.Obj response;
                try {
                    response = bench.handle(c, tRx, in);
                } catch (ProtocolError e) {
                    write(out, e.toJson(c.verb));
                    if (e.closeConnection) {
                        return;
                    }
                    continue;
                } catch (IOException e) {
                    // The stream position is unknown after a failed read or write: answer and close.
                    bench.error("io_error", c.verb + ": " + e);
                    write(out, new ProtocolError("io_error", c.verb + ": " + e, true).toJson(c.verb));
                    return;
                } catch (RuntimeException e) {
                    Log.e(Bench.TAG, c.verb + " failed", e);
                    bench.error("internal_error", c.verb + ": " + e);
                    write(out, new ProtocolError("internal_error", c.verb + ": " + e, true).toJson(c.verb));
                    return;
                }
                write(out, response);
            }
        } catch (IOException e) {
            Log.w(Bench.TAG, "a time-service connection failed", e);
            bench.book.event(Json.obj().put("type", "server").put("event", "connection_failed")
                    .put("detail", e.toString()).put("t_dev_ns", System.nanoTime()));
        }
    }

    private static void write(OutputStream out, Json.Obj response) throws IOException {
        out.write((response.toJson() + "\n").getBytes(StandardCharsets.UTF_8));
        out.flush();
    }

    private static void close(LocalSocket socket) {
        try {
            socket.close();
        } catch (IOException e) {
            Log.w(Bench.TAG, "closing a time-service connection failed", e);
        }
    }
}
