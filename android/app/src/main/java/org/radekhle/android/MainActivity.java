package org.radekhle.android;

import android.content.Intent;
import android.database.Cursor;
import android.net.Uri;
import android.os.Build;
import android.os.Process;
import android.provider.DocumentsContract;
import android.provider.OpenableColumns;
import android.util.Log;

import org.libsdl.app.SDLActivity;

import java.io.File;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.io.OutputStream;

public class MainActivity extends SDLActivity {
    private static final String TAG = "RadekHLE9.0";
    private static final int GAME_FOLDER_REQUEST = 4711;
    private static final int CUSTOM_DRIVER_REQUEST = 4712;
    private static final int ADD_IPA_REQUEST = 4713;
    private static final int ADD_IPA_MESSAGE = 0x8000;
    private static final int PERFORMANCE_MODE_MESSAGE = 0x8001;
    private Object performanceHintSession;

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

    private static File gameFolderTarget() {
        return new File(getContext().getExternalFilesDir(null), "touchHLE_apps");
    }

    private static File customDriverTarget() {
        return new File(getContext().getExternalFilesDir(null), "touchHLE_custom_drivers");
    }

    private static void importSelectedFolder(Uri treeUri) {
        new Thread(() -> {
            int copied = copySelectedFolder(treeUri);

            Log.i(TAG, "Imported " + copied + " files from the selected game folder; restarting RadekHLE9.0 to rescan all games.");
            if (mSingleton != null) {
                mSingleton.runOnUiThread(() -> mSingleton.recreate());
            }
        }, "RadekHLE9.0-game-import").start();
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
        return copyDocumentChildren(childrenUri, treeUri, target, selectedBundle);
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
                if (!copyAll && !isGamePackageName(name)) {
                    Log.i(TAG, "Skipping non-game entry in selected folder: " + name);
                    continue;
                }
                File destination = new File(target, name);
                if (DocumentsContract.Document.MIME_TYPE_DIR.equals(mimeType)) {
                    if (destination.isDirectory() || destination.mkdirs()) {
                        Uri childUri = DocumentsContract.buildChildDocumentsUriUsingTree(treeUri, documentId);
                        copied += copyDocumentChildren(childUri, treeUri, destination, true);
                    } else {
                        Log.e(TAG, "Couldn't create imported game directory: " + destination);
                    }
                } else if (copyDocument(treeUri, documentId, destination)) {
                    copied++;
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
        }, "RadekHLE9.0-game-import").start();
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
        }, "RadekHLE9.0-custom-driver-import").start();
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
