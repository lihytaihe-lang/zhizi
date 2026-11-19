using System;
using System.Collections.Generic;
using System.Windows.Forms;

internal sealed class SyntheticContextCase {
    internal readonly string Id, Prefix, Pinyin, Commit, ControlKind;
    internal readonly int Digit;

    internal SyntheticContextCase(string id, string prefix, string pinyin, string commit, string controlKind, int digit) {
        if (string.IsNullOrEmpty(id) || prefix == null || string.IsNullOrEmpty(commit))
            throw new ArgumentException("Case needs an ID, a prefix, and a committed suffix");
        if (string.IsNullOrEmpty(pinyin) || pinyin.Length > 24)
            throw new ArgumentException("Case needs 1 to 24 lowercase pinyin letters");
        foreach (char letter in pinyin)
            if (letter < 'a' || letter > 'z') throw new ArgumentException("Pinyin must be lowercase ASCII");
        if (digit < 1 || digit > 5) throw new ArgumentOutOfRangeException("digit");
        if (controlKind != "Edit" && controlKind != "RichEdit") throw new ArgumentException("Unknown edit control");
        Id = id;
        Prefix = prefix;
        Pinyin = pinyin;
        Commit = commit;
        ControlKind = controlKind;
        Digit = digit;
    }

    internal string ExpectedDocument { get { return Prefix + Commit; } }
}

internal static class SyntheticContextCases {
    internal static IReadOnlyList<SyntheticContextCase> Create() {
        return Array.AsReadOnly(new[] {
            new SyntheticContextCase("context-slip-edit", "比赛中连续出现了几次不该有的低级", "shiwu", "失误", "Edit", 1),
            new SyntheticContextCase("context-route-edit", "请把备份文件保存到管理员事先", "zhiding", "指定", "Edit", 1),
            new SyntheticContextCase("context-offpage-edit", "调查组正在搜寻黑匣子，以查明飞机", "shishi", "失事", "Edit", 5),
            new SyntheticContextCase("context-treatment-rich", "只压下表面症状，不能根除病因，这种方法只能", "zhibiao", "治标", "RichEdit", 1),
            new SyntheticContextCase("context-cheese-rich", "厨师在披萨上撒了一层可以拉丝的", "zhishi", "芝士", "RichEdit", 5),
            new SyntheticContextCase("context-video-rich", "这台电视兼容多种视频信号", "zhishi", "制式", "RichEdit", 5),
            new SyntheticContextCase("no-context-edit", "", "shiwu", "食物", "Edit", 1),
        });
    }

    internal static void SeedAndFocus(Form owner, TextBoxBase control, SyntheticContextCase specimen) {
        if (owner == null || control == null || specimen == null) throw new ArgumentNullException();
        if (owner.IsDisposed || control.IsDisposed || !owner.IsHandleCreated || !control.IsHandleCreated ||
            owner.InvokeRequired || control.InvokeRequired)
            throw new InvalidOperationException("Seed the test control on its live UI thread");
        Type expected = specimen.ControlKind == "Edit" ? typeof(TextBox) : typeof(RichTextBox);
        if (control.GetType() != expected || control.FindForm() != owner ||
            !control.Visible || !control.Enabled || control.ReadOnly)
            throw new InvalidOperationException("Test case does not own a visible and writable " + specimen.ControlKind);
        var edit = control as TextBox;
        if (edit != null && (edit.Multiline || edit.UseSystemPasswordChar || edit.PasswordChar != '\0' ||
            edit.CharacterCasing != CharacterCasing.Normal))
            throw new InvalidOperationException("Edit must preserve literal input and accept ordinary text");

        control.Text = specimen.Prefix;
        if (!control.Focus()) throw new InvalidOperationException("Test control did not take focus");
        control.Select(control.TextLength, 0);
        if (!control.Focused || !string.Equals(control.Text, specimen.Prefix, StringComparison.Ordinal) ||
            control.SelectionStart != specimen.Prefix.Length || control.SelectionLength != 0)
            throw new InvalidOperationException("Test prefix or caret differs from the fixture " + specimen.Id);
    }

    internal static bool HasExpectedDocument(SyntheticContextCase specimen, TextBoxBase control) {
        return string.Equals(control.Text, specimen.ExpectedDocument, StringComparison.Ordinal);
    }
}
