import java.awt.Color;
import java.awt.Component;
import java.awt.Font;
import java.awt.FontMetrics;
import java.awt.Graphics;
import java.awt.event.KeyAdapter;
import java.awt.event.KeyEvent;

import javax.tv.locator.Locator;
import javax.tv.service.selection.ServiceContextFactory;
import javax.tv.xlet.Xlet;
import javax.tv.xlet.XletContext;
import javax.tv.xlet.XletStateChangeException;

import bdbuild.Titles;

import org.bluray.net.BDLocator;
import org.dvb.ui.FontFactory;
import org.havi.ui.HScene;
import org.havi.ui.HSceneFactory;

public class HelloXlet implements Xlet {
    private XletContext ctx;

    private HScene scene;
    private MenuPanel panel;

    public void initXlet(XletContext ctx) throws XletStateChangeException {
        this.ctx = ctx;
        scene = HSceneFactory.getInstance().getDefaultHScene();
        scene.setLayout(null);

        Font titleFont, buttonFont;
        try {
            FontFactory fonts = new FontFactory();
            titleFont = fonts.createFont("Sans", Font.BOLD, 96);
            buttonFont = fonts.createFont("Sans", Font.BOLD, 48);
        } catch (Exception e) {
            throw new XletStateChangeException("cannot load disc font: " + e);
        }

        panel = new MenuPanel(titleFont, buttonFont);
        panel.setBounds(0, 0, scene.getWidth(), scene.getHeight());
        scene.add(panel);
        scene.addKeyListener(new MenuKeyListener());
    }

    public void startXlet() throws XletStateChangeException {
        scene.setVisible(true);
        scene.requestFocus();
        scene.repaint();
    }

    public void pauseXlet() {
    }

    public void destroyXlet(boolean unconditional) throws XletStateChangeException {
        if (scene != null) {
            scene.setVisible(false);
            HSceneFactory.getInstance().dispose(scene);
            scene = null;
        }
    }

    // Plays the video title, which then jumps back to the menu.
    private void play(int title) {
        try {
            ServiceContextFactory.getInstance().getServiceContext(ctx)
                    .select(new Locator[] { new BDLocator("bd://" + title) });
        } catch (Exception e) {
            e.printStackTrace();
        }
    }

    static class MenuPanel extends Component {
        private final Font titleFont;
        private final Font buttonFont;

        MenuPanel(Font titleFont, Font buttonFont) {
            this.titleFont = titleFont;
            this.buttonFont = buttonFont;
        }

        public void paint(Graphics g) {
            int w = getWidth();
            int h = getHeight();
            g.setColor(Color.black);
            g.fillRect(0, 0, w, h);

            g.setColor(Color.white);
            drawCentered(g, titleFont, "BDJ Lab Demo", w / 2, h / 2 - 120);

            int bw = 420, bh = 110;
            int bx = (w - bw) / 2, by = h / 2 + 20;
            g.fillRect(bx, by, bw, bh);
            g.setColor(Color.black);
            drawCentered(g, buttonFont, "▶ Play", w / 2, by + bh / 2);
        }

        // Draws text centred horizontally on cx, vertically on cy.
        private static void drawCentered(Graphics g, Font font, String text, int cx, int cy) {
            g.setFont(font);
            FontMetrics fm = g.getFontMetrics();
            g.drawString(text, cx - fm.stringWidth(text) / 2,
                         cy + (fm.getAscent() - fm.getDescent()) / 2);
        }
    }

    class MenuKeyListener extends KeyAdapter {
        public void keyPressed(KeyEvent e) {
            if (e.getKeyCode() == KeyEvent.VK_ENTER) {
                play(Titles.VIDEO);
            }
        }
    }
}
