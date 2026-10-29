import java.io.ByteArrayOutputStream;
import java.io.InputStream;
import java.io.OutputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.security.KeyStore;
import java.security.PrivateKey;
import java.security.Signature;
import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.Map;
import java.util.TimeZone;
import java.util.zip.ZipEntry;
import java.util.zip.ZipFile;
import java.util.zip.ZipOutputStream;

import sun.security.pkcs.ContentInfo;
import sun.security.pkcs.PKCS7;
import sun.security.pkcs.SignerInfo;

import static java.nio.charset.StandardCharsets.UTF_8;

/**
 * Turns a jar signed by `jarsigner -sigfile SIG-BD00` into a BD-J signed jar,
 * like SecurityUtil.signWithBDJHeader in the HD Cookbook security tools:
 * adds "BDJ-Signature-Version: 1.0" after Created-By in SIG-BD00.SF, drops
 * SHA1-Digest-Manifest-Main-Attributes, re-signs the .SF and rebuilds SIG-BD00.RSA.
 *
 * usage: BDJSign <jar> <keystore> <storepass> <alias> <entry time, ms since the epoch>
 */
public class BDJSign {
    static final String SF = "META-INF/SIG-BD00.SF";
    static final String RSA = "META-INF/SIG-BD00.RSA";

    public static void main(String[] args) throws Exception {
        Path jar = Paths.get(args[0]);
        char[] pass = args[2].toCharArray();

        KeyStore ks = KeyStore.getInstance(KeyStore.getDefaultType());
        try (InputStream in = Files.newInputStream(Paths.get(args[1]))) {
            ks.load(in, pass);
        }
        PrivateKey key = (PrivateKey) ks.getKey(args[3], pass);
        // Zip entry times are local times: fix the zone so the jar doesn't depend on it.
        TimeZone.setDefault(TimeZone.getTimeZone("UTC"));
        long time = Long.parseLong(args[4]);

        Map<String, byte[]> entries = new LinkedHashMap<>(); // in jar order
        try (ZipFile zip = new ZipFile(jar.toFile())) {
            for (ZipEntry e : Collections.list(zip.entries())) {
                try (InputStream in = zip.getInputStream(e)) {
                    entries.put(e.getName(), readAll(in));
                }
            }
        }
        if (!entries.containsKey(SF) || !entries.containsKey(RSA)) {
            throw new IllegalStateException("no SIG-BD00 signature: sign with jarsigner -sigfile SIG-BD00 first");
        }

        String sf = new String(entries.get(SF), UTF_8).replace("\r\n", "\n");
        String bdjSf = sf.replaceAll("SHA1-Digest-Manifest-Main-Attributes: .*\n", "")
                         .replaceFirst("(Created-By: .*\n)", "$1BDJ-Signature-Version: 1.0\n");
        if (bdjSf.equals(sf)) {
            throw new IllegalStateException("no Created-By line in " + SF);
        }
        byte[] newSf = bdjSf.getBytes(UTF_8);

        Signature signer = Signature.getInstance("SHA1withRSA");
        signer.initSign(key);
        signer.update(newSf);

        PKCS7 block = new PKCS7(entries.get(RSA));
        SignerInfo[] infos = block.getSignerInfos();
        if (infos.length != 1) {
            throw new IllegalStateException("BD-J allows exactly one signer, found " + infos.length);
        }
        SignerInfo si = new SignerInfo(infos[0].getIssuerName(), infos[0].getCertificateSerialNumber(),
                infos[0].getDigestAlgorithmId(), infos[0].getDigestEncryptionAlgorithmId(), signer.sign());
        PKCS7 newBlock = new PKCS7(block.getDigestAlgorithmIds(), new ContentInfo(ContentInfo.DATA_OID, null),
                block.getCertificates(), new SignerInfo[] { si });
        if (newBlock.verify(si, newSf) == null) {
            throw new IllegalStateException("re-signed SIG-BD00.SF does not verify");
        }
        ByteArrayOutputStream rsa = new ByteArrayOutputStream();
        newBlock.encodeSignedData(rsa);

        entries.put(SF, newSf);
        entries.put(RSA, rsa.toByteArray());
        try (OutputStream out = Files.newOutputStream(jar); ZipOutputStream zout = new ZipOutputStream(out)) {
            for (Map.Entry<String, byte[]> e : entries.entrySet()) {
                ZipEntry entry = new ZipEntry(e.getKey());
                entry.setTime(time);
                zout.putNextEntry(entry);
                zout.write(e.getValue());
                zout.closeEntry();
            }
        }
    }

    static byte[] readAll(InputStream in) throws Exception {
        ByteArrayOutputStream out = new ByteArrayOutputStream();
        byte[] buf = new byte[8192];
        for (int n; (n = in.read(buf)) > 0;) {
            out.write(buf, 0, n);
        }
        return out.toByteArray();
    }
}
