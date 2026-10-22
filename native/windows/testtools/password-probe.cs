using System;
using System.Windows.Forms;
using System.Drawing;
using System.Runtime.InteropServices;
class Probe {
 [DllImport("msctf.dll")] static extern int SetInputScope(IntPtr hwnd,int scope);
 [STAThread] static void Main(){
  Application.EnableVisualStyles(); var f=new Form{Text="Zhizi password scope test (synthetic input only)",Width=650,Height=260};
  var a=new TextBox{Left=20,Top=40,Width=580}; var p=new TextBox{Left=20,Top=110,Width=580,UseSystemPasswordChar=true};
  var l=new Label{Left=20,Top=155,Width=600,Text="Normal field above; password field below. Type test text only."};
  f.Controls.AddRange(new Control[]{a,p,l});
  f.Shown+=(s,e)=>{SetInputScope(a.Handle,0); SetInputScope(p.Handle,31);};
  p.TextChanged+=(s,e)=>{l.Text="Password characters: "+p.Text.Length+"; literal nihao: "+(p.Text=="nihao");};
  Application.Run(f);
 }
}
