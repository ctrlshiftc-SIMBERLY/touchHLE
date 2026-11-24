/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 *
 * Parts of this file are derived from SDL 2's Android project template, which
 * has a different license. Please see vendor/SDL/LICENSE.txt for details.
 */
package org.touchhle.android;

import android.content.Intent;
import android.database.Cursor;
import android.net.Uri;
import android.os.Bundle;
import android.provider.OpenableColumns;
import android.widget.Toast;

import org.libsdl.app.SDLActivity;

import java.io.File;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.io.OutputStream;

/**
 * A wrapper class over SDLActivity
 */

public class MainActivity extends SDLActivity {
    @Override
    protected String[] getLibraries() {
        return new String[]{
            "SDL2",
            "touchHLE"
        };
    }

    private static final int PICK_APP_REQUEST = 4242;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        // Import before touchHLE starts, so the app picker sees the file.
        importAppFromIntent(getIntent(), false);
        super.onCreate(savedInstanceState);
        // No apps yet? Offer a file picker so the user can add one without
        // needing a file manager that can reach Android/data.
        if (savedInstanceState == null && !hasAnyApps()) {
            try {
                Intent pick = new Intent(Intent.ACTION_OPEN_DOCUMENT);
                pick.addCategory(Intent.CATEGORY_OPENABLE);
                pick.setType("*/*");
                startActivityForResult(pick, PICK_APP_REQUEST);
                Toast.makeText(this, "Pick your .ipa file to add it to touchHLE",
                        Toast.LENGTH_LONG).show();
            } catch (Exception e) {
                // No document picker available; carry on.
            }
        }
    }

    private boolean hasAnyApps() {
        File dir = new File(getExternalFilesDir(null), "touchHLE_apps");
        String[] names = dir.list();
        if (names == null) {
            return false;
        }
        for (String n : names) {
            String l = n.toLowerCase();
            if (l.endsWith(".ipa") || l.endsWith(".app")) {
                return true;
            }
        }
        return false;
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        if (requestCode == PICK_APP_REQUEST) {
            if (resultCode == RESULT_OK && data != null && data.getData() != null) {
                Intent view = new Intent(Intent.ACTION_VIEW, data.getData());
                if (importAppFromIntent(view, false)) {
                    // touchHLE has already scanned for apps, so restart it.
                    Intent restart = getPackageManager()
                            .getLaunchIntentForPackage(getPackageName());
                    if (restart != null) {
                        restart.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK
                                | Intent.FLAG_ACTIVITY_CLEAR_TASK);
                        startActivity(restart);
                    }
                    Runtime.getRuntime().exit(0);
                }
            }
            return;
        }
        super.onActivityResult(requestCode, resultCode, data);
    }

    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        importAppFromIntent(intent, true);
    }

    /**
     * Copies an .ipa that was opened with or shared to touchHLE into the
     * touchHLE_apps directory.
     */
    private boolean importAppFromIntent(Intent intent, boolean alreadyRunning) {
        if (intent == null) {
            return false;
        }
        Uri uri = null;
        String action = intent.getAction();
        if (Intent.ACTION_VIEW.equals(action)) {
            uri = intent.getData();
        } else if (Intent.ACTION_SEND.equals(action)) {
            uri = intent.getParcelableExtra(Intent.EXTRA_STREAM);
        }
        if (uri == null) {
            return false;
        }
        String name = displayName(uri);
        if (name == null || name.isEmpty()) {
            name = "Imported app.ipa";
        }
        name = name.replace('/', '_');
        if (!name.toLowerCase().endsWith(".ipa")) {
            name = name + ".ipa";
        }
        File dir = new File(getExternalFilesDir(null), "touchHLE_apps");
        dir.mkdirs();
        File dest = new File(dir, name);
        try (InputStream in = getContentResolver().openInputStream(uri);
             OutputStream out = new FileOutputStream(dest)) {
            byte[] buf = new byte[1 << 16];
            int n;
            while ((n = in.read(buf)) > 0) {
                out.write(buf, 0, n);
            }
            String msg = alreadyRunning
                ? "Added " + name + ". Close touchHLE and open it again to see it."
                : "Added " + name + ".";
            Toast.makeText(this, msg, Toast.LENGTH_LONG).show();
        } catch (Exception e) {
            dest.delete();
            Toast.makeText(this, "Couldn't add " + name + ": " + e, Toast.LENGTH_LONG).show();
            intent.setAction(Intent.ACTION_MAIN);
            return false;
        }
        // Don't import the same file again if the activity is recreated.
        intent.setAction(Intent.ACTION_MAIN);
        return true;
    }

    private String displayName(Uri uri) {
        if ("file".equals(uri.getScheme())) {
            return new File(uri.getPath()).getName();
        }
        try (Cursor c = getContentResolver().query(uri,
                new String[]{OpenableColumns.DISPLAY_NAME}, null, null, null)) {
            if (c != null && c.moveToFirst()) {
                return c.getString(0);
            }
        } catch (Exception e) {
            // fall through
        }
        return null;
    }
}
