using System;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;
using Microsoft.Windows.ApplicationModel.DynamicDependency;

namespace WinSpaces.Gui
{
    public static class Program
    {
        [STAThread]
        public static void Main(string[] args)
        {
            WinRT.ComWrappersSupport.InitializeComWrappers();

            uint[] versions = new uint[] { 0x00010006, 0x00010005, 0x00020003 };
            foreach (var ver in versions)
            {
                if (Bootstrap.TryInitialize(ver, out _))
                {
                    break;
                }
            }

            Application.Start((p) => new App());
        }
    }
}
