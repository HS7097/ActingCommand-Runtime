package io.github.hs7097.capturebench.pattern.app;

import android.app.Activity;
import android.content.Intent;
import android.content.pm.ActivityInfo;
import android.content.res.Configuration;
import android.graphics.Point;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.HandlerThread;
import android.os.Looper;
import android.view.Display;
import android.view.FrameMetrics;
import android.view.View;
import android.view.Window;
import android.view.WindowInsets;
import android.view.WindowInsetsController;
import android.view.WindowManager;

import io.github.hs7097.capturebench.pattern.core.Command;
import io.github.hs7097.capturebench.pattern.core.Json;
import io.github.hs7097.capturebench.pattern.core.ProtocolError;

/**
 * The only activity: a full-screen {@link PatternView}, FrameMetrics feeding {@code t_drawn},
 * orientation requests, and the optional launch extras {@code mode} (pattern, flip, step or
 * static) and {@code orient} (PROTOCOL.md, "Install and launch").
 */
public final class PatternActivity extends Activity {
    private static final long SWEEP_MS = 250;

    private Bench bench;
    private PatternView view;
    private Handler handler;
    private HandlerThread metricsThread;
    private Window.OnFrameMetricsAvailableListener metricsListener;
    private String requestedOrientation = "unspecified";

    private final Runnable sweeper = new Runnable() {
        @Override
        public void run() {
            bench.book.sweep(System.nanoTime());
            handler.postDelayed(this, SWEEP_MS);
        }
    };

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        bench = Bench.get(this);
        handler = new Handler(Looper.getMainLooper());
        Window window = getWindow();
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON | WindowManager.LayoutParams.FLAG_FULLSCREEN);
        if (Build.VERSION.SDK_INT >= 28) {
            WindowManager.LayoutParams attributes = window.getAttributes();
            attributes.layoutInDisplayCutoutMode = Build.VERSION.SDK_INT >= 30
                    ? WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_ALWAYS
                    : WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES;
            window.setAttributes(attributes);
        }
        view = new PatternView(this, bench);
        setContentView(view);
        hideSystemBars();
        registerFrameMetrics();
        bench.attach(this);
        applyExtras(getIntent());
        handler.postDelayed(sweeper, SWEEP_MS);
    }

    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        setIntent(intent);
        applyExtras(intent);
    }

    @Override
    public void onWindowFocusChanged(boolean hasFocus) {
        super.onWindowFocusChanged(hasFocus);
        if (hasFocus) {
            hideSystemBars();
        }
    }

    @Override
    public void onConfigurationChanged(Configuration configuration) {
        super.onConfigurationChanged(configuration);
        bench.book.event(Json.obj().put("type", "configuration").put("orientation", orientationName(configuration))
                .put("screen_width_dp", configuration.screenWidthDp).put("screen_height_dp", configuration.screenHeightDp)
                .put("t_dev_ns", System.nanoTime()));
    }

    @Override
    protected void onDestroy() {
        handler.removeCallbacks(sweeper);
        if (metricsListener != null) {
            getWindow().removeOnFrameMetricsAvailableListener(metricsListener);
            metricsListener = null;
        }
        if (metricsThread != null) {
            metricsThread.quitSafely();
            metricsThread = null;
        }
        bench.detach(this);
        super.onDestroy();
    }

    PatternView view() throws ProtocolError {
        if (view == null) {
            throw new ProtocolError("no_view", "the pattern view is not created");
        }
        return view;
    }

    Json.Obj info() {
        Display display = getWindowManager().getDefaultDisplay();
        Point real = new Point();
        display.getRealSize(real);
        Json.Obj displayInfo = Json.obj()
                .put("rotation", display.getRotation())
                .put("real_width", real.x)
                .put("real_height", real.y)
                .put("refresh_hz", Double.valueOf(display.getRefreshRate()));
        Json.Obj out = Bench.ok("INFO")
                .put("instance", bench.instanceId)
                .put("display", displayInfo)
                .put("orientation", Json.obj().put("requested", requestedOrientation)
                        .put("configuration", orientationName(getResources().getConfiguration())));
        Json.Obj state = view.info();
        for (String key : state.keys()) {
            out.put(key, state.get(key));
        }
        return out.put("records", bench.book.stats());
    }

    Json.Obj requestOrientation(String orientation, long tRx) {
        int value;
        if ("landscape".equals(orientation)) {
            value = ActivityInfo.SCREEN_ORIENTATION_LANDSCAPE;
        } else if ("portrait".equals(orientation)) {
            value = ActivityInfo.SCREEN_ORIENTATION_PORTRAIT;
        } else if ("reverse_landscape".equals(orientation)) {
            value = ActivityInfo.SCREEN_ORIENTATION_REVERSE_LANDSCAPE;
        } else if ("reverse_portrait".equals(orientation)) {
            value = ActivityInfo.SCREEN_ORIENTATION_REVERSE_PORTRAIT;
        } else {
            value = ActivityInfo.SCREEN_ORIENTATION_UNSPECIFIED;
        }
        String previous = requestedOrientation;
        setRequestedOrientation(value);
        requestedOrientation = orientation;
        long now = System.nanoTime();
        bench.book.event(Json.obj().put("type", "orient").put("requested", orientation).put("previous", previous)
                .put("t_rx_dev_ns", tRx).put("t_applied_dev_ns", now));
        return Bench.ok("ORIENT").put("requested", orientation).put("previous", previous);
    }

    private void applyExtras(Intent intent) {
        if (intent == null) {
            return;
        }
        long now = System.nanoTime();
        String orient = intent.getStringExtra("orient");
        if (orient != null) {
            if (Command.ORIENTATIONS.contains(orient)) {
                requestOrientation(orient, now);
            } else {
                bench.error("bad_launch_extra", "orient=" + orient + " is not one of " + Command.ORIENTATIONS);
            }
        }
        String mode = intent.getStringExtra("mode");
        if (mode != null) {
            try {
                Command c = Command.parse("MODE " + mode);
                if ("replay".equals(c.mode) || c.slot != null) {
                    throw new ProtocolError("bad_arguments", "the launch extra takes pattern, flip, step or static only");
                }
                view.requestMode(c, now, null, null);
            } catch (ProtocolError e) {
                bench.error("bad_launch_extra", "mode=" + mode + ": " + e.code + ": " + e.getMessage());
            }
        }
    }

    private void hideSystemBars() {
        if (Build.VERSION.SDK_INT >= 30) {
            getWindow().setDecorFitsSystemWindows(false);
            WindowInsetsController controller = getWindow().getInsetsController();
            if (controller != null) {
                controller.hide(WindowInsets.Type.systemBars());
                controller.setSystemBarsBehavior(WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE);
            } else {
                bench.error("system_bars_not_hidden", "the window has no insets controller");
            }
        } else {
            getWindow().getDecorView().setSystemUiVisibility(View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY
                    | View.SYSTEM_UI_FLAG_FULLSCREEN
                    | View.SYSTEM_UI_FLAG_HIDE_NAVIGATION
                    | View.SYSTEM_UI_FLAG_LAYOUT_STABLE
                    | View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN
                    | View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION);
        }
    }

    private void registerFrameMetrics() {
        metricsThread = new HandlerThread("frame-metrics");
        metricsThread.start();
        Handler metricsHandler = new Handler(metricsThread.getLooper());
        Window.OnFrameMetricsAvailableListener listener = new Window.OnFrameMetricsAvailableListener() {
            @Override
            public void onFrameMetricsAvailable(Window window, FrameMetrics metrics, int dropCountSinceLastInvocation) {
                long gpu = Build.VERSION.SDK_INT >= 31 ? metrics.getMetric(FrameMetrics.GPU_DURATION) : -1L;
                bench.book.metrics(
                        metrics.getMetric(FrameMetrics.VSYNC_TIMESTAMP),
                        metrics.getMetric(FrameMetrics.INTENDED_VSYNC_TIMESTAMP),
                        metrics.getMetric(FrameMetrics.TOTAL_DURATION),
                        gpu,
                        metrics.getMetric(FrameMetrics.FIRST_DRAW_FRAME) == 1L,
                        dropCountSinceLastInvocation);
            }
        };
        try {
            getWindow().addOnFrameMetricsAvailableListener(listener, metricsHandler);
            metricsListener = listener;
            bench.book.setMetricsAvailable(true);
        } catch (IllegalStateException e) {
            bench.book.setMetricsAvailable(false);
            bench.error("frame_metrics_unavailable", e.toString());
        }
    }

    private static String orientationName(Configuration configuration) {
        if (configuration.orientation == Configuration.ORIENTATION_LANDSCAPE) {
            return "landscape";
        }
        if (configuration.orientation == Configuration.ORIENTATION_PORTRAIT) {
            return "portrait";
        }
        return "undefined";
    }
}
