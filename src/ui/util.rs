use glib::{
    IsA, ObjectExt,
    subclass::prelude::ClassStruct,
    translate::{IntoGlibPtr, ToGlibPtr, ToGlibPtrMut},
    value::{FromValue, ValueType},
};
use gtk::subclass::prelude::WidgetImpl;

pub trait ObjectExtExt {
    fn property_safe<V: for<'b> FromValue<'b> + 'static>(&self, property_name: &str) -> Option<V>;
}

impl<I: IsA<glib::Object>> ObjectExtExt for I {
    fn property_safe<V: for<'b> FromValue<'b> + 'static>(&self, property_name: &str) -> Option<V> {
        if self.has_property(property_name, None) {
            Some(self.property::<V>(property_name))
        } else {
            None
        }
    }
}

pub trait WidgetExtExt {
    fn style_property<V: for<'b> FromValue<'b> + ValueType + 'static>(&self, property_name: &str) -> V;
}

impl<I: IsA<gtk::Widget>> WidgetExtExt for I {
    // gtk::Widget::style_get_property() is broken: it passes an uninitialized GValue to the FFI,
    // which isn't valid to do: GTK throws a critical error and returns.  So let's implement a more
    // correct one that takes into account the target type.
    fn style_property<V: for<'b> FromValue<'b> + ValueType + 'static>(&self, property_name: &str) -> V {
        let mut value = glib::Value::for_value_type::<V>();
        unsafe {
            gtk::ffi::gtk_widget_style_get_property(
                self.as_ref().to_glib_none().0,
                property_name.to_glib_none().0,
                value.to_glib_none_mut().0,
            );
        }
        value
            .get::<V>()
            .unwrap_or_else(|e| panic!("Failed to get cast value to a different type {e}"))
    }
}

pub trait WidgetClassSubclassExtExt: ClassStruct
where
    <Self as ClassStruct>::Type: WidgetImpl,
{
    fn install_style_property_from_pspec(&mut self, pspec: glib::ParamSpec);
}

impl<T> WidgetClassSubclassExtExt for T
where
    T: ClassStruct,
    <T as ClassStruct>::Type: WidgetImpl,
{
    fn install_style_property_from_pspec(&mut self, pspec: glib::ParamSpec) {
        unsafe {
            // SAFETY:
            // * 'self' is a valid reference to the class struct, which we can cast to a raw
            //   pointer and then to *mut GtkWidgetClass because T::Type: WidgetImpl guarantees
            //   the class is a GtkWidgetClass (or a subclass).
            // * pspec is a valid GParamSpec; we fully transfer ownership to libgtk
            gtk::ffi::gtk_widget_class_install_style_property(self as *mut Self as *mut gtk::ffi::GtkWidgetClass, pspec.into_glib_ptr());
        }
    }
}
