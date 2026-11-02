// SPDX-License-Identifier: AGPL-3.0-only
// Adapted from HS7097/codex-goal-lifecycle@c9538cc5 scripts/runtime/src/bridge/user_input/ClaudeDesktopInput.cs. Copyright (c) 2026 HS7097. MIT: see ../../LICENSE-codex-goal-lifecycle.txt.
//
// Workflow #432 AGT-T1b (contracts/agent-forward.md, "Restoring a stopped session"): the source's
// wake path and nothing else. It brings one conversation up in Claude Desktop and types nothing into
// it: the source's input path, which fills the prompt and presses its button, is left out. The
// message itself goes over the session's inbox once the session has started.
//
// Request on stdin: {"local_session": "local_<uuid>", "account_id": "<uuid>", "wake_only": true}.
// Answer on stdout, one JSON line:
//   {"state": "loaded", "process_id": n}                     exit 0
//   {"state": "queued", "reason": "<word>", "detail": "..."} exit 0, nothing was done
//   {"state": "not_submitted", "error": "...", "problem"?}   exit 12
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using System.Web.Script.Serialization;

// Windows SDK UIAutomationClient.idl interface prefixes; unused slots retain ABI order.
[ComImport, Guid("30cbe57d-d9d0-452a-ab13-7ac5ac4825ee"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
internal interface IAutomation {
    void CompareElements(); void CompareRuntimeIds(); void GetRootElement();
    IElement ElementFromHandle(IntPtr hwnd);
    void ElementFromPoint(); void GetFocusedElement(); void GetRootElementBuildCache();
    void ElementFromHandleBuildCache(); void ElementFromPointBuildCache(); void GetFocusedElementBuildCache();
    void CreateTreeWalker(); void ControlViewWalker(); void ContentViewWalker(); void RawViewWalker();
    void RawViewCondition(); void ControlViewCondition(); void ContentViewCondition(); void CreateCacheRequest();
    void CreateTrueCondition(); void CreateFalseCondition();
    [return: MarshalAs(UnmanagedType.IUnknown)] object CreatePropertyCondition(int property, [MarshalAs(UnmanagedType.Struct)] object value);
    void CreatePropertyConditionEx();
    [return: MarshalAs(UnmanagedType.IUnknown)] object CreateAndCondition([MarshalAs(UnmanagedType.IUnknown)] object a, [MarshalAs(UnmanagedType.IUnknown)] object b);
}
[ComImport, Guid("d22108aa-8ac5-49a5-837b-37bbb3d7591e"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
internal interface IElement {
    void FocusSlot(); void GetRuntimeId();
    IElement FindFirst(int scope, [MarshalAs(UnmanagedType.IUnknown)] object condition);
    IElements FindAll(int scope, [MarshalAs(UnmanagedType.IUnknown)] object condition);
    void FindFirstBuildCache(); void FindAllBuildCache(); void BuildUpdatedCache();
    [return: MarshalAs(UnmanagedType.Struct)] object GetCurrentPropertyValue(int property);
    void GetCurrentPropertyValueEx(); void GetCachedPropertyValue(); void GetCachedPropertyValueEx();
    void GetCurrentPatternAs(); void GetCachedPatternAs();
    [return: MarshalAs(UnmanagedType.IUnknown)] object GetCurrentPattern(int pattern);
}
[ComImport, Guid("14314595-b4bc-4055-95f2-58f2e42c9855"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
internal interface IElements { int Length { get; } IElement GetElement(int index); }
// The value pattern is only read: its first slot, which writes, keeps the ABI order and is never called.
[ComImport, Guid("a94cd8b1-0844-4cd6-9d2d-640537ab39e9"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
internal interface IValue {
    void UnusedSlot();
    string CurrentValue { [return: MarshalAs(UnmanagedType.BStr)] get; }
    bool CurrentIsReadOnly { [return: MarshalAs(UnmanagedType.Bool)] get; }
}

// A bounded Windows accessibility adapter. It never launches a CLI session and never types.
internal static class ClaudeDesktopWake {
    private const int Descendants=4, Edit=50004, Document=50030;
    private const int ControlType=30003, Name=30005, Enabled=30010, Value=30045, LegacyValue=30093;
    private const int ShowNoActivate=4;
    [DllImport("user32.dll")] private static extern bool IsIconic(IntPtr window);
    [DllImport("user32.dll")] private static extern bool ShowWindow(IntPtr window, int command);
    private static Mutex InputLock;
    private static bool lockHeld;
    private static IAutomation Automation;
    private static readonly JavaScriptSerializer Json = new JavaScriptSerializer { MaxJsonLength = 8388608 };

    [MTAThread]
    private static int Main() {
        Console.InputEncoding = new UTF8Encoding(false);
        Console.OutputEncoding = new UTF8Encoding(false);
        try {
            // The source's lock, by the source's name, so a wake never meets the source's own input.
            InputLock=new Mutex(false,"Local\\CGL-ClaudeDesktopInput");
            try { lockHeld=InputLock.WaitOne(0); }
            catch (AbandonedMutexException) { lockHeld=true; }
            if (!lockHeld) throw new WakeQueued("input_busy", "Another native input is using the Claude window");
            var request = Json.Deserialize<Dictionary<string,object>>(Console.In.ReadToEnd());
            if (request==null) throw new InvalidOperationException("The request is empty");
            foreach (var key in request.Keys)
                if (key!="local_session" && key!="account_id" && key!="wake_only")
                    throw new InvalidOperationException("The request holds an unknown member: " + key);
            if (!request.ContainsKey("wake_only") || !(request["wake_only"] is bool) || !(bool)request["wake_only"])
                throw new InvalidOperationException("This helper only restores a session");
            var local = request.ContainsKey("local_session") ? request["local_session"] as string : null;
            Guid parsed;
            if (local==null || !local.StartsWith("local_", StringComparison.Ordinal) || !Guid.TryParse(local.Substring(6), out parsed))
                throw new InvalidOperationException("Invalid local session identity");
            var account = request.ContainsKey("account_id") ? request["account_id"] as string : null;
            if (account==null) throw new InvalidOperationException("The request names no account");
            var target = "https://claude.ai/epitaxy/" + local;
            CheckAccount(account);
            var windows = Process.GetProcessesByName("claude").Where(p => {
                try { return p.MainWindowHandle != IntPtr.Zero && p.MainModule.FileName.IndexOf("AnthropicClaude", StringComparison.OrdinalIgnoreCase) >= 0; }
                catch (System.ComponentModel.Win32Exception e) { throw new InvalidOperationException("Cannot verify Claude window ownership", e); }
            }).ToArray();
            if (windows.Length==0) throw new WakeQueued("no_window", "Claude Desktop shows no main window");
            if (windows.Length!=1) throw new WakeQueued("window_not_unique", "Claude Desktop must expose exactly one main window");
            var process = windows[0];
            var window = process.MainWindowHandle;
            // Restored without taking the focus.
            if (IsIconic(window)) ShowWindow(window, ShowNoActivate);
            Automation=(IAutomation)Activator.CreateInstance(Type.GetTypeFromCLSID(new Guid("ff48dba4-60ef-4201-aa87-54103eef594e")));
            var document = FindDocument(Automation.ElementFromHandle(window), target);
            // The displayed prompt must hold no unsent draft: switching would hide it.
            RequireEmpty(WaitPrompt(window));
            var continuation = "claude://code/continue?session=" + local + "&source=desktop_action";
            Process.Start(new ProcessStartInfo(process.MainModule.FileName, "\"" + continuation + "\"") {
                UseShellExecute = false, CreateNoWindow = true, WindowStyle = ProcessWindowStyle.Hidden
            });
            for (var i=0; i<40 && document==null; i++) {
                Thread.Sleep(250);
                document = FindDocument(Automation.ElementFromHandle(window), target);
            }
            if (document==null) throw new InvalidOperationException("Claude did not display the exact target session within ten seconds");
            CheckAccount(account);
            Console.WriteLine(Json.Serialize(new { state="loaded", process_id=process.Id }));
            return 0;
        } catch (WakeQueued queued) {
            Console.WriteLine(Json.Serialize(new { state="queued", reason=queued.Reason, detail=queued.Message }));
            return 0;
        } catch (AccountChanged changed) {
            Console.WriteLine(Json.Serialize(new { state="not_submitted", problem="account_changed", error=changed.Message }));
            return 12;
        } catch (Exception error) {
            Console.WriteLine(Json.Serialize(new { state="not_submitted", error=error.GetType().Name + ": " + error.Message }));
            return 12;
        } finally {
            if (lockHeld) InputLock.ReleaseMutex();
            if (InputLock!=null) InputLock.Dispose();
        }
    }

    // Nothing was done; the reason is one word the Runtime knows.
    private sealed class WakeQueued : Exception {
        public readonly string Reason;
        public WakeQueued(string reason, string message):base(message){ Reason=reason; }
    }
    private sealed class AccountChanged : Exception {
        public AccountChanged():base("Claude Desktop account changed; the session was not restored"){}
    }
    private static bool IsEnabled(IElement element) { return (bool)element.GetCurrentPropertyValue(Enabled); }
    private static void RequireEmpty(IElement editor) {
        var value=editor.GetCurrentPattern(10002) as IValue;
        if (value==null || value.CurrentIsReadOnly || !IsEnabled(editor))
            throw new InvalidOperationException("Cannot verify an editable Claude prompt");
        // Chromium exposes one paragraph terminator even for an empty contenteditable.
        var draft=value.CurrentValue;
        if (!String.IsNullOrEmpty(draft) && draft!="\n" && draft!="\r\n")
            throw new WakeQueued("draft_present", "An unsent draft is present in the Claude window");
    }
    private static void CheckAccount(string expected) {
        var path=Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData),"Claude","config.json");
        var config=Json.Deserialize<Dictionary<string,object>>(File.ReadAllText(path,Encoding.UTF8));
        if (config==null || !config.ContainsKey("lastKnownAccountUuid") || config["lastKnownAccountUuid"] as string!=expected)
            throw new AccountChanged();
    }
    private static IElement FindDocument(IElement root, string target) {
        var documents = root.FindAll(Descendants,Automation.CreatePropertyCondition(ControlType,Document));
        var matches = new List<IElement>();
        for (var i=0;i<documents.Length;i++) {
            var document=documents.GetElement(i);
            if (document.GetCurrentPropertyValue(Value) as string==target || document.GetCurrentPropertyValue(LegacyValue) as string==target) matches.Add(document);
        }
        if (matches.Count>1) throw new InvalidOperationException("Multiple matching Claude documents; the target is ambiguous");
        return matches.Count==1 ? matches[0] : null;
    }
    // The prompt of the whole window: the conversation it shows now, before any switch.
    private static IElement WaitPrompt(IntPtr window) {
        for (var i=0;i<40;i++) {
            var controls=Automation.ElementFromHandle(window).FindAll(Descendants,Condition(Edit,"Prompt"));
            if (controls.Length>1) throw new InvalidOperationException("Multiple Claude prompt controls; the draft cannot be checked");
            if (controls.Length==1) return controls.GetElement(0);
            if (i==0) Console.Error.WriteLine("WARNING: Claude native editor is loading; waiting up to ten seconds before the restore");
            Thread.Sleep(250);
        }
        throw new InvalidOperationException("Claude native editor was unavailable for ten seconds; the session was not restored");
    }
    private static object Condition(int type,string name) {
        return Automation.CreateAndCondition(Automation.CreatePropertyCondition(ControlType,type),Automation.CreatePropertyCondition(Name,name));
    }
}
