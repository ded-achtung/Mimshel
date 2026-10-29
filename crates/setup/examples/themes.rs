//! Print the installed themes offered by the settings window.

fn main() {
    println!("Темы GTK: {:?}", mimshel_setup::themes::gtk_themes());
    println!("Темы значков: {:?}", mimshel_setup::themes::icon_themes());
    println!(
        "Темы курсоров: {:?}",
        mimshel_setup::themes::cursor_themes()
    );
}
