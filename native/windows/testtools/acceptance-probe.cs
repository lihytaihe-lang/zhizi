// Native TSF acceptance: physical keys and mouse into this process's own controls.
// Compile with ProfileActivation.cs, SyntheticContextCases.cs, System.Windows.Forms, Drawing and Web.Extensions.
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Drawing;
using System.Drawing.Imaging;
using System.IO;
using System.Runtime.InteropServices;
using System.Security.Cryptography;
using System.Text;
using System.Web.Script.Serialization;
using System.Windows.Forms;

class AcceptanceProbe : Form {
    [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] static extern bool SetForegroundWindow(IntPtr window);
    [DllImport("user32.dll")] static extern uint SendInput(uint count, INPUT[] inputs, int size);
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr window, out RECT rect);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr window);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr window, out uint process);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumWindow callback, IntPtr data);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetClassName(IntPtr window, StringBuilder text, int capacity);
    [DllImport("user32.dll")] static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] static extern uint GetDpiForWindow(IntPtr window);
    [DllImport("user32.dll")] static extern IntPtr WindowFromPoint(POINT point);
    [DllImport("msctf.dll")] static extern int SetInputScope(IntPtr window, int scope);
    delegate bool EnumWindow(IntPtr window, IntPtr data);
    [StructLayout(LayoutKind.Sequential)] struct RECT { public int left,top,right,bottom; }
    [StructLayout(LayoutKind.Sequential)] struct POINT { public int x,y; public POINT(int a,int b){x=a;y=b;} }
    [StructLayout(LayoutKind.Sequential)] struct KEYBDINPUT { public ushort vk,scan; public uint flags,time; public UIntPtr extra; }
    [StructLayout(LayoutKind.Sequential)] struct MOUSEINPUT { public int x,y; public uint data,flags,time; public UIntPtr extra; }
    [StructLayout(LayoutKind.Explicit)] struct UNION { [FieldOffset(0)] public KEYBDINPUT key; [FieldOffset(0)] public MOUSEINPUT mouse; }
    [StructLayout(LayoutKind.Sequential)] struct INPUT { public uint type; public UNION value; }
    class TestCase {
        public string id,raw,expected,action;
        public bool secure,focusFromComposition;
        public SyntheticContextCase context;
    }

    readonly string output,mode;
    readonly TextBox normal=new TextBox(), password=new TextBox();
    readonly RichTextBox rich;
    readonly Label status=new Label();
    readonly Timer timer=new Timer();
    readonly List<object> records=new List<object>();
    readonly List<TestCase> cases=new List<TestCase>();
    int index=-1,character,stage,activation; bool passed=true,finished;
    DateTime next,started;

    [STAThread] static void Main(string[] args) {
        if(args.Length!=2 || (args[1]!="current" && args[1]!="rollback" && args[1]!="restored" && args[1]!="context")) { Environment.Exit(2); return; }
        Application.EnableVisualStyles();
        Application.SetCompatibleTextRenderingDefault(false);
        try { Application.Run(new AcceptanceProbe(args[0],args[1])); }
        catch(Exception error) {
            Directory.CreateDirectory(args[0]);
            File.WriteAllText(Path.Combine(args[0],"fatal.txt"),error.ToString(),Encoding.UTF8);
            Environment.Exit(1);
        }
    }
    AcceptanceProbe(string path,string variant) {
        output=Path.GetFullPath(path); mode=variant;
        if(!output.StartsWith(@"H:\zhizi\diagnostics\",StringComparison.OrdinalIgnoreCase)) throw new ArgumentException("isolated diagnostics output required");
        Directory.CreateDirectory(output);
        Text="知字原生 TSF 验收 · 合成输入"; Width=Math.Min(1140,Screen.PrimaryScreen.WorkingArea.Width-60); Height=520;
        StartPosition=FormStartPosition.CenterScreen;
        normal.SetBounds(24,70,Width-70,36); normal.Font=new Font("Microsoft YaHei UI",16);
        password.SetBounds(24,280,Width-70,36); password.Font=normal.Font; password.UseSystemPasswordChar=true;
        status.SetBounds(24,340,Width-70,100); status.Font=new Font("Microsoft YaHei UI",11);
        Controls.AddRange(new Control[]{normal,password,status});
        if(mode=="context") {
            rich=new RichTextBox();
            rich.SetBounds(24,145,Width-70,78);
            rich.Font=normal.Font;
            Controls.Add(rich);
            foreach(var fixture in SyntheticContextCases.Create())
                cases.Add(new TestCase{id=fixture.Id,raw=fixture.Pinyin,expected=fixture.ExpectedDocument,action="digit",context=fixture});
        }
        cases.Add(new TestCase{id="normal-digit",raw="nihao",expected="你好",action="digit"});
        cases.Add(new TestCase{id="normal-mouse",raw="nihao",expected="你好",action="mouse"});
        if(mode=="current") cases.Add(new TestCase{id="generation-only",raw="wmxcqcfb",expected="我们下次去吃饭吧",action="digit"});
        cases.Add(new TestCase{id="secure-literal",raw="nihao",expected="nihao",secure=true,action="none"});
        if(mode=="current" || mode=="context") cases.Add(new TestCase{id="focus-to-secure",raw="nihao",expected="nihao",secure=true,focusFromComposition=true,action="none"});
        timer.Interval=100; timer.Tick+=Tick;
        Shown+=(s,e)=>{
            SetInputScope(normal.Handle,0); SetInputScope(password.Handle,31);
            if(rich!=null)SetInputScope(rich.Handle,0);
            normal.Focus(); SetForegroundWindow(Handle);
            activation=ZhiziProfile.Activate();
            status.Text="TSF ActivateProfile HRESULT: 0x"+activation.ToString("X8")+"；等待真实输入法加载。";
            started=DateTime.UtcNow; stage=-1; next=started; timer.Start();
        };
    }
    void Guard(){ if(GetForegroundWindow()!=Handle) throw new InvalidOperationException("probe lost foreground; no input sent"); }
    static bool HostReady(string log) {
        if(!File.Exists(log))return false;
        try {
            using(var stream=new FileStream(log,FileMode.Open,FileAccess.Read,FileShare.ReadWrite|FileShare.Delete))
            using(var reader=new StreamReader(stream,Encoding.UTF8))return reader.ReadToEnd().Contains("Host ready");
        }catch(IOException){return false;}
    }
    void Key(ushort vk) {
        Guard();
        INPUT down=new INPUT{type=1,value=new UNION{key=new KEYBDINPUT{vk=vk}}};
        INPUT up=new INPUT{type=1,value=new UNION{key=new KEYBDINPUT{vk=vk,flags=2}}};
        if(SendInput(2,new INPUT[]{down,up},Marshal.SizeOf(typeof(INPUT)))!=2) throw new InvalidOperationException("SendInput rejected physical key");
    }
    IntPtr Panel() {
        IntPtr found=IntPtr.Zero; uint pid=(uint)Process.GetCurrentProcess().Id;
        EnumWindows((window,data)=>{uint owner; GetWindowThreadProcessId(window,out owner);
            if(owner==pid && IsWindowVisible(window)){var name=new StringBuilder(128);GetClassName(window,name,128);if(name.ToString()=="ZhiziCandidatePanelV1"){found=window;return false;}} return true;},IntPtr.Zero);
        return found;
    }
    void ClickFirst() {
        Guard(); IntPtr panel=Panel(); RECT r;
        if(panel==IntPtr.Zero || !GetWindowRect(panel,out r)) throw new InvalidOperationException("own visible candidate panel missing");
        double scale=GetDpiForWindow(panel)/96.0; int x=r.left+(int)(40*scale),y=r.top+(int)(48*scale);
        if(WindowFromPoint(new POINT(x,y))!=panel) throw new InvalidOperationException("candidate click is occluded; no mouse input sent");
        SetCursorPos(x,y);
        INPUT down=new INPUT{type=0,value=new UNION{mouse=new MOUSEINPUT{flags=2}}};
        INPUT up=new INPUT{type=0,value=new UNION{mouse=new MOUSEINPUT{flags=4}}};
        if(SendInput(2,new INPUT[]{down,up},Marshal.SizeOf(typeof(INPUT)))!=2) throw new InvalidOperationException("SendInput rejected mouse click");
    }
    void Capture(string name) {
        Guard(); Rectangle r=Bounds;
        using(var bitmap=new Bitmap(r.Width,r.Height)) { using(var g=Graphics.FromImage(bitmap)){g.CopyFromScreen(r.Location,Point.Empty,r.Size);} bitmap.Save(Path.Combine(output,name+".png"),ImageFormat.Png); }
    }
    void BeginCase() {
        index++; if(index==cases.Count){Finish(null);return;}
        var c=cases[index]; normal.Clear();password.Clear(); normal.Focus();
        character=0;status.Text=c.id+"：合成拼音 "+c.raw;
        if(c.context!=null) {
            SyntheticContextCases.SeedAndFocus(this, ContextTarget(c.context), c.context);
            stage=0;next=DateTime.UtcNow.AddMilliseconds(200);
        }
        else if(c.focusFromComposition){Key(0x4E);Key(0x49);stage=4;next=DateTime.UtcNow.AddMilliseconds(400);}
        else {if(c.secure)password.Focus();stage=0;next=DateTime.UtcNow.AddMilliseconds(200);}
    }
    TextBoxBase ContextTarget(SyntheticContextCase context) {
        return context.ControlKind=="RichEdit" ? (TextBoxBase)rich : normal;
    }
    void Tick(object sender,EventArgs args) {
        if(finished || DateTime.UtcNow<next)return;
        try {
            Guard();
            if(stage==-1) {
                if(activation<0)throw new InvalidOperationException("TSF process activation failed");
                string log=Path.Combine(Environment.GetEnvironmentVariable("APPDATA"),@"Zhizi\shell-status.log");
                if(HostReady(log)){BeginCase();return;}
                if((DateTime.UtcNow-started).TotalSeconds>25)throw new TimeoutException("actual TSF Host did not become ready");
                return;
            }
            var c=cases[index];
            if(c.context!=null && !ContextTarget(c.context).Focused)
                throw new InvalidOperationException("Context test lost its edit control before selection");
            if(stage==4){password.Focus();stage=0;next=DateTime.UtcNow.AddMilliseconds(200);return;}
            if(stage==0) {
                if(character<c.raw.Length){Key((ushort)char.ToUpperInvariant(c.raw[character++]));return;}
                stage=1;next=DateTime.UtcNow.AddMilliseconds(650);return;
            }
            if(stage==1) {
                if(c.context!=null && Panel()==IntPtr.Zero)
                    throw new InvalidOperationException("Context test has no visible candidate panel");
                Capture(c.id+"-before-selection");
                if(c.action=="digit")Key((ushort)(0x30+(c.context==null?1:c.context.Digit))); else if(c.action=="mouse")ClickFirst();
                stage=2;next=DateTime.UtcNow.AddMilliseconds(350);return;
            }
            if(stage==2) {
                string actual=c.context!=null?ContextTarget(c.context).Text:c.secure?password.Text:normal.Text;
                bool ok=c.context!=null?SyntheticContextCases.HasExpectedDocument(c.context, ContextTarget(c.context)):actual==c.expected;
                records.Add(new {id=c.id,raw=c.raw,expected=c.expected,actual=actual,normal=normal.Text,password=password.Text,passed=ok,
                    control=c.context==null?(c.secure?"Password":"Edit"):c.context.ControlKind,
                    prefix=c.context==null?"":c.context.Prefix,digit=c.context==null?(c.action=="digit"?(int?)1:null):c.context.Digit,
                    expectedSuffix=c.context==null?c.expected:c.context.Commit});
                passed &= ok;Capture(c.id+"-committed");BeginCase();
            }
        } catch(Exception error){Finish(error);}
    }
    void Finish(Exception error) {
        if(finished)return;finished=true;timer.Stop();
        var modules=new List<object>();
        foreach(ProcessModule module in Process.GetCurrentProcess().Modules) {
            if(string.Equals(module.ModuleName,"zhizi_tsf.dll",StringComparison.OrdinalIgnoreCase)) {
                string digest;using(var stream=new FileStream(module.FileName,FileMode.Open,FileAccess.Read,FileShare.ReadWrite|FileShare.Delete))using(var hash=SHA256.Create()){digest=BitConverter.ToString(hash.ComputeHash(stream)).Replace("-","").ToLowerInvariant();}
                modules.Add(new {path=module.FileName,version=module.FileVersionInfo.FileVersion,sha256=digest});
            }
        }
        passed &= error==null && modules.Count==1 && records.Count==cases.Count;
        var result=new {mode=mode,passed=passed,error=error==null?null:error.ToString(),activationHRESULT=activation,process=Process.GetCurrentProcess().Id,modules=modules,cases=records,physicalInput=true,processLocalProfile=true,isolatedAppData=Environment.GetEnvironmentVariable("APPDATA")};
        File.WriteAllText(Path.Combine(output,"RESULT.json"),new JavaScriptSerializer().Serialize(result),new UTF8Encoding(false));
        Environment.ExitCode=passed?0:1;Close();
    }
}
