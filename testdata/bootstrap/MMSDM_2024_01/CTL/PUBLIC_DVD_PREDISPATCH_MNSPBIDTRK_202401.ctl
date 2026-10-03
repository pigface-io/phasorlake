-- ************************************************************
-- Title:	PUBLIC_DVD_PREDISPATCH_MNSPBIDTRK_202401.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS PREDISPATCH_MNSPBIDTRK owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_DVD_PREDISPATCH_MNSPBIDTRK_202401010000.CSV
APPEND INTO TABLE PREDISPATCH_MNSPBIDTRK
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
PREDISPATCHSEQNO CHAR(20),
LINKID CHAR(10),
PERIODID CHAR(20),
PARTICIPANTID CHAR(10),
SETTLEMENTDATE DATE "yyyy/mm/dd hh24:mi:ss",
OFFERDATE TIMESTAMP(3),
VERSIONNO FLOAT EXTERNAL,
DATETIME DATE "yyyy/mm/dd hh24:mi:ss",
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss")
-- End of Script
