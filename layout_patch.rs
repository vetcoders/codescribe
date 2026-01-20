        // --- LAYOUT --- (Split View)
        let header_height = 30.0;
        let input_area_height = 40.0;
        let left_panel_width = 450.0;
        let right_panel_x = left_panel_width;
        let right_panel_width = window_width - left_panel_width;
        let checkbox_width = 50.0;
        let send_width = 60.0;
        let attach_width = 30.0;
        let input_margin = 8.0;
        let controls_y = window_height - header_height - input_area_height;
        // Left Panel Status
        let status_frame = CGRect { origin: CGPoint { x: 0.0, y: window_height - header_height }, size: CGSize { width: left_panel_width, height: header_height } };
        let status_field: Id = msg_send![ns_text_field, alloc];
        let status_field: Id = msg_send![status_field, initWithFrame: status_frame];
        let _: () = msg_send![status_field, setBezeled: false];
        let _: () = msg_send![status_field, setDrawsBackground: true];
        let _: () = msg_send![status_field, setEditable: false];
