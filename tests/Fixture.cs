using System;
using System.Collections.Generic;
using System.Drawing;
using System.IO;
using System.Runtime.InteropServices;
using System.Threading;
using System.Web.Script.Serialization;
using System.Windows.Forms;

// Disposable test application. File IPC is intentionally confined to this fixture.
class LabForm : Form {
    readonly string root;
    readonly JavaScriptSerializer json = new JavaScriptSerializer();
    readonly TextBox a = new TextBox(), b = new TextBox();
    readonly Button first = new Button(), second = new Button();
    readonly Label status = new Label();
    readonly Panel pane = new Panel();
    readonly CheckBox toggle = new CheckBox();
    Form cover;
    TextBox injected;
    int clicksA, clicksB, lastCommand, ticks;
    string lastError = "";
    readonly System.Windows.Forms.Timer timer = new System.Windows.Forms.Timer();
    [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
    protected override bool ShowWithoutActivation { get { return true; } }
    public LabForm(string dir) {
        root = dir;
        Text = "OCU Native Lab - disposable target";
        Name = "OcuLabTarget";
        StartPosition = FormStartPosition.Manual;
        Location = new Point(80,80); ClientSize = new Size(610,420);
        BackColor = Color.FromArgb(18,62,90); ForeColor = Color.White;
        Font = new Font("Segoe UI", 11);
        var title = new Label { Text="OCU LAB / BACKGROUND TEST", Location=new Point(20,15), AutoSize=true };
        Controls.Add(title);
        pane.Name="TestPanel"; pane.AccessibleName="TestPanel";
        pane.Location=new Point(20,55); pane.Size=new Size(565,290);
        Controls.Add(pane);
        a.Name="FieldA"; a.AccessibleName="FieldA"; a.Location=new Point(10,12); a.Size=new Size(515,28); a.Text="A_SEED"; a.TabIndex=0;
        b.Name="FieldB"; b.AccessibleName="FieldB"; b.Location=new Point(10,62); b.Size=new Size(515,28); b.Text="B_SEED"; b.TabIndex=1;
        first.Name="CommitA"; first.AccessibleName="CommitA"; first.Text="CommitA"; first.Location=new Point(10,115); first.Size=new Size(150,40); first.TabIndex=2;
        second.Name="CommitB"; second.AccessibleName="CommitB"; second.Text="CommitB"; second.Location=new Point(185,115); second.Size=new Size(150,40); second.TabIndex=3;
        toggle.Name="TestToggle"; toggle.AccessibleName="TestToggle"; toggle.Text="TestToggle"; toggle.Location=new Point(10,180); toggle.Size=new Size(220,30); toggle.TabIndex=4;
        pane.Controls.AddRange(new Control[]{a,b,first,second,toggle});
        first.Click += delegate { clicksA++; Save(); };
        second.Click += delegate { clicksB++; Save(); };
        toggle.CheckedChanged += delegate { Save(); };
        status.Location=new Point(20,360); status.Size=new Size(565,40); Controls.Add(status);
        timer.Interval=100; timer.Tick += delegate { Pump(); }; timer.Start();
        Shown += delegate { Save(); };
        FormClosed += delegate { if(cover!=null) cover.Close(); };
    }
    void Save() {
        try {
            status.Text="A clicks="+clicksA+" / B clicks="+clicksB+" / IPC="+lastCommand;
            var data=new Dictionary<string,object> {
                {"pid",System.Diagnostics.Process.GetCurrentProcess().Id}, {"hwnd",Handle.ToInt64()},
                {"a",a.Text},{"b",b.Text},{"aClicks",clicksA},{"bClicks",clicksB},
                {"toggle",toggle.Checked},{"injected",injected==null?null:injected.Text},
                {"focus",ActiveControl==null?"":ActiveControl.Name},{"bFocused",b.Focused},
                {"foreground",GetForegroundWindow().ToInt64()},{"coverHwnd",cover==null?0:cover.Handle.ToInt64()},
                {"command",lastCommand},{"ticks",ticks},{"error",lastError},
                {"x",Left},{"y",Top},{"width",Width},{"height",Height}
            };
            File.WriteAllText(Path.Combine(root,"fixture-state.tmp"),json.Serialize(data));
            File.Copy(Path.Combine(root,"fixture-state.tmp"),Path.Combine(root,"fixture-state.json"),true);
        } catch(IOException) {}
    }
    void Pump() {
        ticks++;
        string path=Path.Combine(root,"fixture-command.json");
        if(File.Exists(path)) {
            try {
                var cmd=json.Deserialize<Dictionary<string,object>>(File.ReadAllText(path));
                int id=Convert.ToInt32(cmd["id"]);
                if(id>lastCommand) {
                    lastCommand=id; lastError="";
                    string op=Convert.ToString(cmd["op"]);
                    if(op=="reset") { a.Text="A_SEED"; b.Text="B_SEED"; clicksA=clicksB=0; toggle.Checked=false; if(injected!=null){pane.Controls.Remove(injected);injected.Dispose();injected=null;} pane.Controls.SetChildIndex(a,0); pane.Controls.SetChildIndex(b,1); }
                    else if(op=="focus-b") { Activate(); b.Focus(); b.SelectionStart=2; b.SelectionLength=3; }
                    else if(op=="cover") {
                        if(cover==null||cover.IsDisposed) {
                            cover=new Form(); cover.Text="OCU Native Lab - cover"; cover.StartPosition=FormStartPosition.Manual;
                            cover.BackColor=Color.Magenta; cover.ForeColor=Color.Black;
                            cover.Controls.Add(new Label {Text="OCCLUDER / USER WORKSPACE",AutoSize=true,Location=new Point(25,40),Font=new Font("Segoe UI",18)});
                        }
                        cover.Bounds=Bounds; cover.Show(); cover.Activate(); cover.BringToFront();
                    }
                    else if(op=="uncover") { if(cover!=null) cover.Hide(); }
                    else if(op=="insert") {
                        injected=new TextBox(); injected.Name="InsertedField"; injected.AccessibleName="InsertedField";
                        injected.Text="DO_NOT_TOUCH"; injected.Location=new Point(10,235);injected.Size=new Size(515,28);
                        pane.Controls.Add(injected);pane.Controls.SetChildIndex(injected,0);
                    }
                    else if(op=="swap-buttons") { pane.Controls.SetChildIndex(second,pane.Controls.GetChildIndex(first)); }
                    else if(op=="move") { Location=new Point(Left+80,Top+35); }
                    else if(op=="minimize") { WindowState=FormWindowState.Minimized; }
                    else if(op=="restore") { WindowState=FormWindowState.Normal; }
                    else if(op=="hang") { Save(); Thread.Sleep(6500); }
                    else if(op=="close") { Save(); Close(); return; }
                    else { lastError="unknown command: "+op; }
                }
            } catch(Exception e) { lastError=e.GetType().Name+": "+e.Message; }
        }
        Save();
    }
    [STAThread] static void Main(string[] args) {
        Application.EnableVisualStyles(); Application.SetCompatibleTextRenderingDefault(false);
        Application.Run(new LabForm(args[0]));
    }
}
