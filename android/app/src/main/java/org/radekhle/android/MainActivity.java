package org.radekhle.android;

import android.content.Intent;
import android.database.Cursor;
import android.net.Uri;
import android.provider.DocumentsContract;
import android.util.Log;

import org.libsdl.app.SDLActivity;

import java.io.File;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.io.OutputStream;

public class MainActivity extends SDLActivity {
    private static final String TAG = "RadekHLE";
    private static final int GAME_FOLDER_REQUEST = 4711;

    @Override
    protected String[] getLibraries() {
        return new String[]{
            "SDL2",
            "radekhle"
        };
    }

    private static File gameFolderTarget() {
        return new File(getContext().getExternalFilesDir(null), "touchHLE_apps");
    }

    private static void importSelectedFolder(Uri treeUri) {
        new Thread(() -> {
            int copied = copySelectedFolder(treeUri);

            Log.i(TAG, "Imported " + copied + " files from the selected game folder; restarting RadekHLE to rescan all games.");
            if (mSingleton != null) {
                mSingleton.runOnUiThread(() -> mSingleton.recreate());
            }
        }, "RadekHLE-game-import").start();
    }

    private static int copySelectedFolder(Uri treeUri) {
        File target = gameFolderTarget();
        if (!target.exists() && !target.mkdirs()) {
            Log.e(TAG, "Couldn't create game folder: " + target);
            return 0;
        }
        String documentId = DocumentsContract.getTreeDocumentId(treeUri);
        Uri childrenUri = DocumentsContract.buildChildDocumentsUriUsingTree(treeUri, documentId);
        return copyDocumentChildren(childrenUri, treeUri, target);
    }

    private static int copyDocumentChildren(Uri childrenUri, Uri treeUri, File target) {
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
                        copied += copyDocumentChildren(childUri, treeUri, destination);
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
            try (OutputStream output = new FileOutputStream(temporary)) {
                byte[] buffer = new byte[1024 * 1024];
                int count;
                while ((count = input.read(buffer)) != -1) output.write(buffer, 0, count);
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

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        if (requestCode != GAME_FOLDER_REQUEST || resultCode != RESULT_OK || data == null || data.getData() == null) return;
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
            Uri uri = Uri.parse(url);
            if ("touchhle".equalsIgnoreCase(uri.getScheme()) && "game-folder".equalsIgnoreCase(uri.getHost())) {
                Intent picker = new Intent(Intent.ACTION_OPEN_DOCUMENT_TREE);
                picker.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION
                    | Intent.FLAG_GRANT_WRITE_URI_PERMISSION
                    | Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION
                    | Intent.FLAG_GRANT_PREFIX_URI_PERMISSION);
                mSingleton.startActivityForResult(picker, GAME_FOLDER_REQUEST);
                return 0;
            }
            return SDLActivity.openURL(url);
        } catch (Exception ex) {
            Log.e(TAG, "Couldn't open URL: " + url, ex);
            return -1;
        }
    }
}
