package com.rua.mobile;

import android.app.Activity;
import android.os.Bundle;
import android.widget.Button;
import android.widget.EditText;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.TextView;

/** Standalone harmless target: no dependency on the instrumentation target's Kotlin runtime. */
public final class PhoneFixtureJavaActivity extends Activity {
    private int clicks;
    @Override public void onCreate(Bundle state) {
        super.onCreate(state);
        LinearLayout column = new LinearLayout(this);
        column.setOrientation(LinearLayout.VERTICAL);
        column.setPadding(30, 180, 30, 30);
        TextView counter = new TextView(this);
        counter.setText("clicks=0"); counter.setTextSize(24);
        Button button = new Button(this);
        button.setText("Increment");
        button.setOnClickListener(v -> counter.setText("clicks=" + (++clicks)));
        button.setOnLongClickListener(v -> { counter.setText("long-pressed"); return true; });
        EditText input = new EditText(this);
        input.setHint("Test input"); input.setContentDescription("Phone fixture input");
        column.addView(counter); column.addView(button); column.addView(input);
        for (int i=0;i<30;i++) {
            TextView row = new TextView(this); row.setText("Fixture row " + i);
            row.setTextSize(22); row.setPadding(0,25,0,25); column.addView(row);
        }
        ScrollView scroll = new ScrollView(this); scroll.addView(column); setContentView(scroll);
    }
}
