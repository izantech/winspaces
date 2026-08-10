using Microsoft.UI.Xaml;

namespace WinSpaces.Gui
{
    public class App : Application
    {
        public static Window? MainWindow { get; private set; }

        protected override void OnLaunched(LaunchActivatedEventArgs args)
        {
            MainWindow = new MainWindow();
            MainWindow.Activate();
        }
    }
}
