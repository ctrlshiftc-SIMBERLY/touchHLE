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

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        // Import before touchHLE starts, so the app picker sees the file.
        importAppFromIntent(getIntent(), false);
        super.onCreate(savedInstanceState);
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
    private void importAppFromIntent(Intent intent, boolean alreadyRunning) {
        if (intent == null) {
            return;
        }
        Uri uri = null;
        String action = intent.getAction();
        if (Intent.ACTION_VIEW.equals(action)) {
            uri = intent.getData();
        } else if (Intent.ACTION_SEND.equals(action)) {
            uri = intent.getParcelableExtra(Intent.EXTRA_STREAM);
        }
        if (uri == null) {
            return;
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
        }
        // Don't import the same file again if the activity is recreated.
        intent.setAction(Intent.ACTION_MAIN);
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
