use std::ffi::CString;

use glib::{
    IsA, ObjectExt, ObjectType, StaticType,
    subclass::prelude::ClassStruct,
    translate::{FromGlibPtrNone, IntoGlib, IntoGlibPtr, ToGlibPtr, ToGlibPtrMut},
    value::{FromValue, ValueType},
};
use gtk::{gdk, subclass::prelude::WidgetImpl, traits::StyleContextExt};

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

pub(crate) fn style_property_value_for_type<V: for<'b> FromValue<'b> + ValueType + 'static>(
    widget_type: glib::Type,
    property_name: &str,
) -> Option<V> {
    if !widget_type.is_a(gtk::Widget::static_type()) {
        None
    } else {
        // SAFETY: 'widget_type' is a valid GObject type, as checked above.
        let klass = Some(unsafe { glib::gobject_ffi::g_type_class_ref(widget_type.into_glib()) }).filter(|ptr| !ptr.is_null())?;

        let name = CString::new(property_name).ok()?;
        // SAFETY: 'klass' is not NULL, and it refers to a GtkWidget subclass's class.
        let ffi_pspec =
            Some(unsafe { gtk::ffi::gtk_widget_class_find_style_property(klass as *mut gtk::ffi::GtkWidgetClass, name.as_ptr()) })
                .filter(|ptr| !ptr.is_null());

        // SAFETY: 'klass' is valid.
        unsafe { glib::gobject_ffi::g_type_class_unref(klass) };

        let ffi_pspec = ffi_pspec?;
        // SAFETY: 'ffi_pspec' is not NULL and is valid.
        let pspec = unsafe { glib::ParamSpec::from_glib_none(ffi_pspec) };

        let widget_path = gtk::WidgetPath::new();
        widget_path.append_type(widget_type);

        let ctx = gtk::StyleContext::new();
        ctx.set_path(&widget_path);
        if let Some(screen) = gdk::Screen::default() {
            ctx.set_screen(&screen);
        }

        let value = glib::Value::from_type(pspec.type_());
        // SAFETY: 'ctx' is non-NULL and valid, 'name' is valid, and 'value' is initialized to the
        // correct type.
        unsafe {
            gtk::ffi::gtk_style_context_get_style_property(ctx.as_ptr(), name.as_ptr(), value.as_ptr());
        };

        value.get::<V>().ok()
    }
}
