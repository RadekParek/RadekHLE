package org.radekhle.android;

import android.Manifest;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.database.Cursor;
import android.graphics.ImageFormat;
import android.hardware.camera2.CameraAccessException;
import android.hardware.camera2.CameraCaptureSession;
import android.hardware.camera2.CameraCharacteristics;
import android.hardware.camera2.CameraDevice;
import android.hardware.camera2.CameraManager;
import android.hardware.camera2.CaptureRequest;
import android.media.AudioFormat;
import android.media.AudioRecord;
import android.media.Image;
import android.media.ImageReader;
import android.media.MediaRecorder;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.HandlerThread;
import android.os.Process;
import android.provider.DocumentsContract;
import android.provider.OpenableColumns;
import android.util.Log;
import android.util.Size;
import android.view.Surface;

import org.libsdl.app.SDLActivity;

import java.io.File;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.io.OutputStream;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Collections;
import java.util.Comparator;
import java.util.List;

public class MainActivity extends SDLActivity {
    private static final String TAG = "RadekHLE9.1";
    private static final int GAME_FOLDER_REQUEST = 4711;
    private static final int CUSTOM_DRIVER_REQUEST = 4712;
    private static final int ADD_IPA_REQUEST = 4713;
    private static final int ADD_IPA_MESSAGE = 0x8000;
    private static final int PERFORMANCE_MODE_MESSAGE = 0x8001;
    private static final int GAME_FOLDER_MESSAGE = 0x8002;
    private static final int CUSTOM_DRIVER_MESSAGE = 0x8003;
    private static final int MEDIA_PERMISSION_REQUEST = 4714;
    private static final int CAMERA_WIDTH = 640;
    private static final int CAMERA_HEIGHT = 480;
    private Object performanceHintSession;
    private final Object captureLock = new Object();
    private HandlerThread cameraThread;
    private Handler cameraHandler;
    private CameraDevice cameraDevice;
    private CameraCaptureSession cameraSession;
    private ImageReader cameraReader;
    private AudioRecord audioRecord;
    private Thread audioThread;
    private volatile boolean captureRunning;
    private long lastCameraWriteNanos;
    private File captureDirectory;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        requestMediaPermissions();
    }

    @Override
    protected void onResume() {
        super.onResume();
        if (hasPermission(Manifest.permission.CAMERA) || hasPermission(Manifest.permission.RECORD_AUDIO)) {
            startNativeCapture();
        }
    }

    @Override
    protected void onPause() {
        stopNativeCapture();
        super.onPause();
    }

    private boolean hasPermission(String permission) {
        return Build.VERSION.SDK_INT < 23
                || checkSelfPermission(permission) == PackageManager.PERMISSION_GRANTED;
    }

    private void requestMediaPermissions() {
        if (Build.VERSION.SDK_INT < 23) {
            startNativeCapture();
            return;
        }
        ArrayList<String> missing = new ArrayList<>();
        if (!hasPermission(Manifest.permission.CAMERA)) missing.add(Manifest.permission.CAMERA);
        if (!hasPermission(Manifest.permission.RECORD_AUDIO)) missing.add(Manifest.permission.RECORD_AUDIO);
        if (Build.VERSION.SDK_INT < 33 && !hasPermission(Manifest.permission.READ_EXTERNAL_STORAGE)) {
            missing.add(Manifest.permission.READ_EXTERNAL_STORAGE);
        }
        if (Build.VERSION.SDK_INT < 29 && !hasPermission(Manifest.permission.WRITE_EXTERNAL_STORAGE)) {
            missing.add(Manifest.permission.WRITE_EXTERNAL_STORAGE);
        }
        if (missing.isEmpty()) {
            startNativeCapture();
        } else {
            requestPermissions(missing.toArray(new String[0]), MEDIA_PERMISSION_REQUEST);
        }
    }

    @Override
    public void onRequestPermissionsResult(int requestCode, String[] permissions, int[] grantResults) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults);
        if (requestCode == MEDIA_PERMISSION_REQUEST) startNativeCapture();
    }

    private void startNativeCapture() {
        synchronized (captureLock) {
            if (captureRunning) return;
            captureRunning = true;
            captureDirectory = new File(getFilesDir(), "radekhle_capture");
            if (!captureDirectory.exists() && !captureDirectory.mkdirs()) {
                Log.e(TAG, "Couldn't create native capture directory: " + captureDirectory);
                captureRunning = false;
                return;
            }
        }
        writeCaptureStatus();
        if (hasPermission(Manifest.permission.CAMERA)) startCameraCapture();
        if (hasPermission(Manifest.permission.RECORD_AUDIO)) startMicrophoneCapture();
    }

    private void stopNativeCapture() {
        synchronized (captureLock) {
            if (!captureRunning) return;
            captureRunning = false;
        }
        closeCameraCapture();
        AudioRecord recorder = audioRecord;
        audioRecord = null;
        if (recorder != null) {
            try { recorder.stop(); } catch (Exception ignored) { }
            recorder.release();
        }
        Thread thread = audioThread;
        audioThread = null;
        if (thread != null && thread != Thread.currentThread()) {
            try { thread.join(500); } catch (InterruptedException ignored) { Thread.currentThread().interrupt(); }
        }
        writeCaptureStatus();
    }

    private void writeCaptureStatus() {
        if (captureDirectory == null) return;
        File target = new File(captureDirectory, "status");
        File temporary = new File(captureDirectory, "status.part");
        String status = "camera=" + (hasPermission(Manifest.permission.CAMERA) ? "1" : "0")
                + "\nmicrophone=" + (hasPermission(Manifest.permission.RECORD_AUDIO) ? "1" : "0")
                + "\nstorage=" + (Build.VERSION.SDK_INT >= 33 || hasPermission(Manifest.permission.READ_EXTERNAL_STORAGE) ? "1" : "0") + "\n";
        try (FileOutputStream output = new FileOutputStream(temporary, false)) {
            output.write(status.getBytes(StandardCharsets.UTF_8));
            output.flush();
            if (target.exists()) target.delete();
            temporary.renameTo(target);
        } catch (Exception ex) {
            Log.w(TAG, "Couldn't update native capture status", ex);
        }
    }

    private void startCameraCapture() {
        if (cameraThread != null || Build.VERSION.SDK_INT < 21) return;
        lastCameraWriteNanos = 0L;
        cameraThread = new HandlerThread("RadekHLE9.1-camera");
        cameraThread.start();
        cameraHandler = new Handler(cameraThread.getLooper());
        try {
            CameraManager manager = (CameraManager) getSystemService(CAMERA_SERVICE);
            String cameraId = null;
            Size outputSize = new Size(CAMERA_WIDTH, CAMERA_HEIGHT);
            for (String id : manager.getCameraIdList()) {
                CameraCharacteristics characteristics = manager.getCameraCharacteristics(id);
                Integer facing = characteristics.get(CameraCharacteristics.LENS_FACING);
                if (facing != null && facing == CameraCharacteristics.LENS_FACING_FRONT) continue;
                android.hardware.camera2.params.StreamConfigurationMap map =
                        characteristics.get(CameraCharacteristics.SCALER_STREAM_CONFIGURATION_MAP);
                if (map != null) {
                    Size[] sizes = map.getOutputSizes(ImageFormat.YUV_420_888);
                    if (sizes != null && sizes.length > 0) {
                        outputSize = chooseCameraSize(sizes);
                    }
                }
                cameraId = id;
                break;
            }
            if (cameraId == null) {
                Log.w(TAG, "No Android camera is available");
                return;
            }
            final Size size = outputSize;
            cameraReader = ImageReader.newInstance(size.getWidth(), size.getHeight(), ImageFormat.YUV_420_888, 2);
            ImageReader reader = cameraReader;
            cameraReader.setOnImageAvailableListener((ImageReader imageReader) -> {
                Image image = null;
                try {
                    image = imageReader.acquireLatestImage();
                    if (image != null && captureRunning) writeCameraFrame(image);
                } catch (Exception ex) {
                    Log.w(TAG, "Native camera frame capture failed", ex);
                } finally {
                    if (image != null) image.close();
                }
            }, cameraHandler);
            if (hasPermission(Manifest.permission.CAMERA)) {
                manager.openCamera(cameraId, new CameraDevice.StateCallback() {
                    @Override public void onOpened(CameraDevice device) {
                        cameraDevice = device;
                        createCameraSession(device, reader.getSurface());
                    }
                    @Override public void onDisconnected(CameraDevice device) {
                        device.close();
                        cameraDevice = null;
                    }
                    @Override public void onError(CameraDevice device, int error) {
                        Log.w(TAG, "Android camera open failed: " + error);
                        device.close();
                        cameraDevice = null;
                    }
                }, cameraHandler);
            }
        } catch (Exception ex) {
            Log.w(TAG, "Couldn't start native Android camera", ex);
        }
    }

    private static Size chooseCameraSize(Size[] sizes) {
        List<Size> candidates = new ArrayList<>();
        Collections.addAll(candidates, sizes);
        candidates.sort(Comparator.comparingLong(size -> Math.abs((long) size.getWidth() * size.getHeight() - (long) CAMERA_WIDTH * CAMERA_HEIGHT)));
        for (Size size : candidates) {
            if (size.getWidth() <= 1280 && size.getHeight() <= 1280) return size;
        }
        return candidates.get(0);
    }

    private void createCameraSession(CameraDevice device, Surface surface) {
        try {
            device.createCaptureSession(Collections.singletonList(surface), new CameraCaptureSession.StateCallback() {
                @Override public void onConfigured(CameraCaptureSession session) {
                    cameraSession = session;
                    try {
                        CaptureRequest.Builder request = device.createCaptureRequest(CameraDevice.TEMPLATE_PREVIEW);
                        request.addTarget(surface);
                        request.set(CaptureRequest.CONTROL_AF_MODE, CaptureRequest.CONTROL_AF_MODE_CONTINUOUS_PICTURE);
                        session.setRepeatingRequest(request.build(), null, cameraHandler);
                    } catch (CameraAccessException ex) {
                        Log.w(TAG, "Couldn't start camera preview", ex);
                    }
                }
                @Override public void onConfigureFailed(CameraCaptureSession session) {
                    Log.w(TAG, "Android camera capture session configuration failed");
                }
            }, cameraHandler);
        } catch (CameraAccessException ex) {
            Log.w(TAG, "Couldn't create camera capture session", ex);
        }
    }

    private void closeCameraCapture() {
        CameraCaptureSession session = cameraSession;
        cameraSession = null;
        if (session != null) session.close();
        CameraDevice device = cameraDevice;
        cameraDevice = null;
        if (device != null) device.close();
        ImageReader reader = cameraReader;
        cameraReader = null;
        if (reader != null) reader.close();
        HandlerThread thread = cameraThread;
        cameraThread = null;
        cameraHandler = null;
        if (thread != null) thread.quitSafely();
    }

    private void writeCameraFrame(Image image) throws Exception {
        long now = System.nanoTime();
        if (now - lastCameraWriteNanos < 66_000_000L) return;
        lastCameraWriteNanos = now;
        int width = image.getWidth();
        int height = image.getHeight();
        byte[] nv21 = imageToNv21(image);
        ByteBuffer header = ByteBuffer.allocate(24).order(ByteOrder.LITTLE_ENDIAN);
        header.putInt(0x52484346);
        header.putInt(width);
        header.putInt(height);
        header.putLong(System.nanoTime());
        header.putInt(nv21.length);
        File target = new File(captureDirectory, "camera.nv21");
        File temporary = new File(captureDirectory, "camera.nv21.part");
        try (FileOutputStream output = new FileOutputStream(temporary, false)) {
            output.write(header.array());
            output.write(nv21);
            output.flush();
            if (target.exists()) target.delete();
            temporary.renameTo(target);
        }
    }

    private static byte[] imageToNv21(Image image) {
        int width = image.getWidth();
        int height = image.getHeight();
        byte[] yuv = new byte[width * height + width * height / 2];
        copyPlane(image.getPlanes()[0], width, height, yuv, 0, 1);
        byte[] u = new byte[width * height / 4];
        byte[] v = new byte[width * height / 4];
        copyPlane(image.getPlanes()[1], width / 2, height / 2, u, 0, 1);
        copyPlane(image.getPlanes()[2], width / 2, height / 2, v, 0, 1);
        int offset = width * height;
        for (int i = 0; i < u.length && offset + i * 2 + 1 < yuv.length; i++) {
            yuv[offset + i * 2] = v[i];
            yuv[offset + i * 2 + 1] = u[i];
        }
        return yuv;
    }

    private static void copyPlane(Image.Plane plane, int width, int height, byte[] output, int outputOffset, int outputPixelStride) {
        ByteBuffer buffer = plane.getBuffer().duplicate();
        int rowStride = plane.getRowStride();
        int pixelStride = plane.getPixelStride();
        for (int row = 0; row < height; row++) {
            for (int column = 0; column < width; column++) {
                int source = row * rowStride + column * pixelStride;
                int destination = outputOffset + (row * width + column) * outputPixelStride;
                if (source < buffer.limit() && destination < output.length) output[destination] = buffer.get(source);
            }
        }
    }

    private void startMicrophoneCapture() {
        if (audioThread != null || Build.VERSION.SDK_INT < 23) return;
        int minimum = AudioRecord.getMinBufferSize(44100, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT);
        if (minimum <= 0) {
            Log.w(TAG, "Android microphone returned no usable buffer size");
            return;
        }
        try {
            audioRecord = new AudioRecord(MediaRecorder.AudioSource.DEFAULT, 44100,
                    AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT, Math.max(minimum * 2, 4096));
            AudioRecord recorder = audioRecord;
            recorder.startRecording();
            audioThread = new Thread(() -> {
                File target = new File(captureDirectory, "microphone.pcm");
                try (FileOutputStream output = new FileOutputStream(target, false)) {
                    ByteBuffer header = ByteBuffer.allocate(16).order(ByteOrder.LITTLE_ENDIAN);
                    header.putInt(0x52484d46);
                    header.putInt(44100);
                    header.putInt(1);
                    header.putInt(16);
                    output.write(header.array());
                    byte[] buffer = new byte[Math.max(minimum, 4096)];
                    while (captureRunning && recorder == audioRecord) {
                        int count = recorder.read(buffer, 0, buffer.length, AudioRecord.READ_BLOCKING);
                        if (count > 0) {
                            output.write(buffer, 0, count);
                            output.flush();
                        }
                    }
                } catch (Exception ex) {
                    Log.w(TAG, "Native microphone capture stopped", ex);
                }
            }, "RadekHLE9.1-microphone");
            audioThread.start();
        } catch (Exception ex) {
            Log.w(TAG, "Couldn't start native Android microphone", ex);
            audioRecord = null;
        }
    }

    @Override
    protected String[] getLibraries() {
        return new String[]{
            "c++_shared",
            "SDL2",
            "radekhle"
        };
    }

    @Override
    protected boolean onUnhandledMessage(int message, Object data) {
        if (message == ADD_IPA_MESSAGE) {
            runOnUiThread(MainActivity::openIpaPicker);
            return true;
        }
        if (message == PERFORMANCE_MODE_MESSAGE) {
            int flags = data instanceof Integer ? (Integer) data : 0;
            runOnUiThread(() -> applyPerformanceMode(flags));
            return true;
        }
        if (message == GAME_FOLDER_MESSAGE) {
            runOnUiThread(MainActivity::openGameFolderPicker);
            return true;
        }
        if (message == CUSTOM_DRIVER_MESSAGE) {
            runOnUiThread(MainActivity::openCustomDriverPicker);
            return true;
        }
        return super.onUnhandledMessage(message, data);
    }

    private void applyPerformanceMode(int flags) {
        boolean highPerformance = (flags & 1) != 0;
        boolean maxClocks = (flags & 2) != 0;
        boolean enabled = highPerformance || maxClocks;
        if (Build.VERSION.SDK_INT >= 24) {
            getWindow().setSustainedPerformanceMode(enabled);
        }
        if (enabled) {
            getWindow().addFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        } else {
            getWindow().clearFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        }
        updatePerformanceHintSession(enabled, maxClocks);
        if (Build.VERSION.SDK_INT >= 30 && enabled) {
            float refreshRate = getWindow().getWindowManager().getDefaultDisplay().getRefreshRate();
            if (refreshRate > 0.0f) {
                android.view.WindowManager.LayoutParams attributes = getWindow().getAttributes();
                attributes.preferredRefreshRate = refreshRate;
                getWindow().setAttributes(attributes);
            }
        }
        Log.i(TAG, "Native sustained-performance hint "
                + (enabled ? "enabled" : "disabled")
                + "; max-clocks request=" + maxClocks
                + " (Android governors still control the actual CPU/GPU clocks)");
    }

    private void updatePerformanceHintSession(boolean enabled, boolean maxClocks) {
        if (Build.VERSION.SDK_INT < 31) return;
        try {
            if (!enabled) {
                if (performanceHintSession != null) {
                    performanceHintSession.getClass().getMethod("close").invoke(performanceHintSession);
                    performanceHintSession = null;
                }
                return;
            }
            if (performanceHintSession == null) {
                Object manager = getSystemService("performance_hint");
                if (manager != null) {
                    performanceHintSession = manager.getClass()
                            .getMethod("createHintSession", int[].class, long.class)
                            .invoke(manager, new int[]{Process.myTid()}, maxClocks ? 8_333_333L : 16_666_667L);
                }
            }
            if (performanceHintSession != null) {
                performanceHintSession.getClass()
                        .getMethod("updateTargetWorkDuration", long.class)
                        .invoke(performanceHintSession, maxClocks ? 8_333_333L : 16_666_667L);
            }
        } catch (Exception ex) {
            Log.w(TAG, "Android performance hint session is unavailable", ex);
            performanceHintSession = null;
        }
    }

    private static void openIpaPicker() {
        if (mSingleton == null) {
            Log.e(TAG, "Couldn't open game picker because the SDL activity is not ready");
            return;
        }
        Intent picker = new Intent(Intent.ACTION_OPEN_DOCUMENT);
        picker.setType("*/*");
        picker.putExtra(Intent.EXTRA_MIME_TYPES, new String[]{
            "application/zip", "application/x-zip-compressed", "application/octet-stream"
        });
        picker.addCategory(Intent.CATEGORY_OPENABLE);
        picker.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION | Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION);
        launchFilePicker(picker, ADD_IPA_REQUEST);
    }

    private static void openGameFolderPicker() {
        Intent picker = new Intent(Intent.ACTION_OPEN_DOCUMENT_TREE);
        picker.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION
            | Intent.FLAG_GRANT_WRITE_URI_PERMISSION
            | Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION
            | Intent.FLAG_GRANT_PREFIX_URI_PERMISSION);
        launchFilePicker(picker, GAME_FOLDER_REQUEST);
    }

    private static void openCustomDriverPicker() {
        Intent picker = new Intent(Intent.ACTION_OPEN_DOCUMENT);
        picker.setType("*/*");
        picker.putExtra(Intent.EXTRA_MIME_TYPES, new String[]{"application/zip", "application/x-zip-compressed", "application/octet-stream"});
        picker.addCategory(Intent.CATEGORY_OPENABLE);
        picker.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION | Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION);
        launchFilePicker(picker, CUSTOM_DRIVER_REQUEST);
    }

    private static File gameFolderTarget() {
        return new File(getContext().getExternalFilesDir(null), "touchHLE_apps");
    }

    private static File customDriverTarget() {
        return new File(getContext().getExternalFilesDir(null), "touchHLE_custom_drivers");
    }

    private static void importSelectedFolder(Uri treeUri) {
        new Thread(() -> {
            int copied = copySelectedFolder(treeUri);

            Log.i(TAG, "Imported " + copied + " files from the selected game folder; restarting RadekHLE9.1 to rescan all games.");
            if (mSingleton != null) {
                mSingleton.runOnUiThread(() -> mSingleton.recreate());
            }
        }, "RadekHLE9.1-game-import").start();
    }

    private static int copySelectedFolder(Uri treeUri) {
        File target = gameFolderTarget();
        if (!target.exists() && !target.mkdirs()) {
            Log.e(TAG, "Couldn't create game folder: " + target);
            return 0;
        }
        String documentId = DocumentsContract.getTreeDocumentId(treeUri);
        Uri childrenUri = DocumentsContract.buildChildDocumentsUriUsingTree(treeUri, documentId);
        String selectedName = selectedDocumentName(treeUri);
        boolean selectedBundle = isGamePackageName(selectedName);
        if (selectedBundle && selectedName != null) {
            File bundleTarget = new File(target, selectedName);
            if (!bundleTarget.isDirectory() && !bundleTarget.mkdirs()) {
                Log.e(TAG, "Couldn't create imported game bundle directory: " + bundleTarget);
                return 0;
            }
            return copyDocumentChildren(childrenUri, treeUri, bundleTarget, true);
        }
        return copyDocumentChildren(childrenUri, treeUri, target, false);
    }

    private static boolean isGamePackageName(String name) {
        if (name == null) return false;
        String lower = name.toLowerCase(java.util.Locale.ROOT);
        return lower.endsWith(".ipa") || lower.endsWith(".app") || lower.endsWith(".zip");
    }

    private static int copyDocumentChildren(Uri childrenUri, Uri treeUri, File target, boolean copyAll) {
        String[] projection = {
            DocumentsContract.Document.COLUMN_DOCUMENT_ID,
            DocumentsContract.Document.COLUMN_DISPLAY_NAME,
            DocumentsContract.Document.COLUMN_MIME_TYPE
        };
        int copied = 0;
        try (Cursor cursor = getContext().getContentResolver().query(childrenUri, projection, null, null, null)) {
            if (cursor == null) return 0;
            int idColumn = cursor.getColumnIndexOrThrow(DocumentsContract.Document.COLUMN_DOCUMENT_ID);
            int nameColumn = cursor.getColumnIndexOrThrow(DocumentsContract.Document.COLUMN_DISPLAY_NAME);
            int mimeColumn = cursor.getColumnIndexOrThrow(DocumentsContract.Document.COLUMN_MIME_TYPE);
            while (cursor.moveToNext()) {
                String documentId = cursor.getString(idColumn);
                String name = cursor.getString(nameColumn);
                String mimeType = cursor.getString(mimeColumn);
                if (name == null || name.isEmpty() || name.equals(".") || name.equals("..")) continue;
                File destination = new File(target, name);
                if (DocumentsContract.Document.MIME_TYPE_DIR.equals(mimeType)) {
                    if (destination.isDirectory() || destination.mkdirs()) {
                        Uri childUri = DocumentsContract.buildChildDocumentsUriUsingTree(treeUri, documentId);
                        copied += copyDocumentChildren(childUri, treeUri, destination, true);
                    } else {
                        Log.e(TAG, "Couldn't create imported game directory: " + destination);
                    }
                } else {
                    if (!copyAll && !isGamePackageName(name)) {
                        Log.i(TAG, "Skipping non-game entry in selected folder: " + name);
                        continue;
                    }
                    if (copyDocument(treeUri, documentId, destination)) copied++;
                }
            }
        } catch (Exception ex) {
            Log.e(TAG, "Couldn't read selected game folder", ex);
        }
        return copied;
    }

    private static boolean copyDocument(Uri treeUri, String documentId, File destination) {
        Uri documentUri = DocumentsContract.buildDocumentUriUsingTree(treeUri, documentId);
        File temporary = new File(destination.getPath() + ".radekhle-part");
        try (InputStream input = getContext().getContentResolver().openInputStream(documentUri)) {
            if (input == null) return false;
            if (temporary.exists() && !temporary.delete()) {
                Log.e(TAG, "Couldn't replace partial imported game file: " + temporary);
                return false;
            }
            try (FileOutputStream output = new FileOutputStream(temporary)) {
                byte[] buffer = new byte[1024 * 1024];
                int count;
                while ((count = input.read(buffer)) != -1) output.write(buffer, 0, count);
                output.flush();
                output.getFD().sync();
            }
            if (destination.exists() && !destination.delete()) {
                Log.e(TAG, "Couldn't replace imported game file: " + destination);
                temporary.delete();
                return false;
            }
            if (!temporary.renameTo(destination)) {
                Log.e(TAG, "Couldn't publish imported game file: " + destination);
                temporary.delete();
                return false;
            }
            return true;
        } catch (Exception ex) {
            temporary.delete();
            Log.e(TAG, "Couldn't copy selected game file: " + destination, ex);
            return false;
        }
    }

    private static void importSelectedIpa(Uri uri) {
        new Thread(() -> {
            File target = gameFolderTarget();
            if (!target.exists() && !target.mkdirs()) {
                Log.e(TAG, "Couldn't create game folder: " + target);
                return;
            }
            String name = selectedDocumentName(uri);
            if (name == null || name.isEmpty()) name = "game.ipa";
            if (!name.toLowerCase().endsWith(".ipa")) name += ".ipa";
            File destination = new File(target, name);
            if (copyDocumentUri(uri, destination)) {
                Log.i(TAG, "Imported game: " + name + "; keeping the native app picker alive so Rust can rescan it.");
            }
        }, "RadekHLE9.1-game-import").start();
    }

    private static void importSelectedCustomDriver(Uri uri) {
        new Thread(() -> {
            File target = customDriverTarget();
            if (!target.exists() && !target.mkdirs()) {
                Log.e(TAG, "Couldn't create custom-driver folder: " + target);
                return;
            }
            String name = selectedDocumentName(uri);
            if (name == null || !name.toLowerCase().endsWith(".zip")) {
                Log.e(TAG, "Selected custom driver is not a ZIP file: " + name);
                return;
            }
            File destination = new File(target, name);
            if (copyDocumentUri(uri, destination)) {
                Log.i(TAG, "Imported custom driver ZIP: " + name);
                if (mSingleton != null) {
                    mSingleton.runOnUiThread(() -> mSingleton.recreate());
                }
            }
        }, "RadekHLE9.1-custom-driver-import").start();
    }

    private static String selectedDocumentName(Uri uri) {
        try (Cursor cursor = getContext().getContentResolver().query(uri,
                new String[]{OpenableColumns.DISPLAY_NAME}, null, null, null)) {
            if (cursor != null && cursor.moveToFirst()) return cursor.getString(0);
        } catch (Exception ex) {
            Log.e(TAG, "Couldn't read selected document name", ex);
        }
        return null;
    }

    private static boolean copyDocumentUri(Uri uri, File destination) {
        File temporary = new File(destination.getPath() + ".radekhle-part");
        try (InputStream input = getContext().getContentResolver().openInputStream(uri)) {
            if (input == null) return false;
            if (temporary.exists() && !temporary.delete()) return false;
            try (FileOutputStream output = new FileOutputStream(temporary)) {
                byte[] buffer = new byte[1024 * 1024];
                int count;
                while ((count = input.read(buffer)) != -1) output.write(buffer, 0, count);
                output.flush();
                output.getFD().sync();
            }
            if (destination.exists() && !destination.delete()) {
                temporary.delete();
                return false;
            }
            if (!temporary.renameTo(destination)) {
                temporary.delete();
                return false;
            }
            return true;
        } catch (Exception ex) {
            temporary.delete();
            Log.e(TAG, "Couldn't copy selected custom driver: " + destination, ex);
            return false;
        }
    }

    private static void launchFilePicker(Intent picker, int requestCode) {
        if (mSingleton == null) {
            Log.e(TAG, "Couldn't open file picker because the SDL activity is not ready");
            return;
        }
        mSingleton.runOnUiThread(() -> {
            try {
                mSingleton.startActivityForResult(picker, requestCode);
            } catch (Exception ex) {
                Log.e(TAG, "Couldn't open file picker", ex);
            }
        });
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        if (resultCode != RESULT_OK || data == null || data.getData() == null) return;
        if (requestCode == ADD_IPA_REQUEST) {
            importSelectedIpa(data.getData());
            return;
        }
        if (requestCode == CUSTOM_DRIVER_REQUEST) {
            importSelectedCustomDriver(data.getData());
            return;
        }
        if (requestCode != GAME_FOLDER_REQUEST) return;
        Uri treeUri = data.getData();
        try {
            getContentResolver().takePersistableUriPermission(treeUri,
                Intent.FLAG_GRANT_READ_URI_PERMISSION | Intent.FLAG_GRANT_WRITE_URI_PERMISSION);
        } catch (Exception ignored) {
        }
        importSelectedFolder(treeUri);
    }

    public static int openURL(String url) {
        try {
            if (mSingleton == null) {
                Log.e(TAG, "Couldn't open URL because the SDL activity is not ready: " + url);
                return -1;
            }
            Uri uri = Uri.parse(url);
            if ("touchhle".equalsIgnoreCase(uri.getScheme()) && "game-folder".equalsIgnoreCase(uri.getHost())) {
                Intent picker = new Intent(Intent.ACTION_OPEN_DOCUMENT_TREE);
                picker.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION
                    | Intent.FLAG_GRANT_WRITE_URI_PERMISSION
                    | Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION
                    | Intent.FLAG_GRANT_PREFIX_URI_PERMISSION);
                launchFilePicker(picker, GAME_FOLDER_REQUEST);
                return 0;
            }
            if ("touchhle".equalsIgnoreCase(uri.getScheme()) && "custom-driver".equalsIgnoreCase(uri.getHost())) {
                Intent picker = new Intent(Intent.ACTION_OPEN_DOCUMENT);
                picker.setType("*/*");
                picker.putExtra(Intent.EXTRA_MIME_TYPES, new String[]{"application/zip", "application/x-zip-compressed", "application/octet-stream"});
                picker.addCategory(Intent.CATEGORY_OPENABLE);
                picker.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION | Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION);
                launchFilePicker(picker, CUSTOM_DRIVER_REQUEST);
                return 0;
            }
            return SDLActivity.openURL(url);
        } catch (Exception ex) {
            Log.e(TAG, "Couldn't open URL: " + url, ex);
            return -1;
        }
    }
}
